//! Deluge-BSP SD card driver.
//!
//! Exposes an async interface for reading and writing 512-byte sectors on the
//! Deluge's SD card (SDHI port 1).
//!
//! Dual-target: on the firmware (`target_os = "none"`) this drives the real
//! SDHI1 controller (`device`); on host/QEMU (`target_os != "none"`) there is
//! no SDHI hardware, so the same public API ([`init`], [`read_sectors`],
//! [`write_sectors`], [`is_ready`], [`is_inserted`], [`is_write_protected`],
//! [`total_sectors`]) is instead backed by a small file-backed disk image
//! (`host`), so callers (e.g. a FatFS diskio shim) need no `#[cfg]` of their
//! own.
//!
//! ## The card protocol (device)
//!
//! `rza1_hal::sdhi::card` runs the SD v2 bring-up (CMD0 … CMD6 High-Speed) and the block
//! transfers. This module adds the Deluge's policy around it: SDHI1 and its pins, DMA channels
//! and interrupts; the socket's inverted write-protect polarity; one lock around each card
//! operation; a statically allocated DMA bounce buffer of `SD_DMA_MAX_SECTORS` blocks behind the
//! uncached mirror; and the wake-up retries a cold card needs ([`init`] tries the protocol 16 times
//! over about 1.3 s and returns as soon as it succeeds).
//!
//! ## Usage
//!
//! ```ignore
//! sd::init().await.expect("SD init failed");
//! let mut buf = [0u8; 512];
//! sd::read_sector(0, &mut buf).await.expect("read failed");
//! ```

// ---------------------------------------------------------------------------
// Error type (shared across device + host)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdError {
    /// Low-level SDHI hardware error. Device-only.
    #[cfg(target_os = "none")]
    Hardware(rza1_hal::sdhi::SdhiError),
    /// Card not present.
    NoCard,
    /// Unsupported card type (e.g. MMC, old SD v1 without sane CMD8 response).
    UnsupportedCard,
    /// Protocol violation — unexpected response.
    Protocol,
    /// Driver not initialized; call [`init`] first.
    NotInitialized,
    /// Host file-backed I/O failure (open/seek/read/write on the backing
    /// image). Host-only.
    #[cfg(not(target_os = "none"))]
    HostIo(std::io::ErrorKind),
}

#[cfg(target_os = "none")]
impl From<rza1_hal::sdhi::SdhiError> for SdError {
    fn from(e: rza1_hal::sdhi::SdhiError) -> Self {
        SdError::Hardware(e)
    }
}

#[cfg(target_os = "none")]
impl From<rza1_hal::sdhi::card::CardError> for SdError {
    fn from(e: rza1_hal::sdhi::card::CardError) -> Self {
        use rza1_hal::sdhi::card::CardError;
        match e {
            CardError::Sdhi(e) => SdError::Hardware(e),
            CardError::Unsupported => SdError::UnsupportedCard,
            _ => SdError::Protocol,
        }
    }
}

#[cfg(target_os = "none")]
pub use device::{
    controller, hw_ready, init, init_hw, invalidate, is_hc, is_hs, is_inserted, is_ready,
    is_write_protected, read_sectors, take_card_detect_events, total_sectors, write_sectors,
};
// The embedded-sdmmc adapters only exist when the `fat` feature pulls that crate in.
#[cfg(all(target_os = "none", feature = "fat"))]
pub use device::{DelugeBlockDevice, DelugeTimeSource, PartitionShim};
#[cfg(not(target_os = "none"))]
pub use host::{
    MemoryDiskError, dump_memory_disk, init, invalidate, is_inserted, is_ready, is_write_protected,
    read_sectors, take_card_detect_events, total_sectors, use_memory_disk, write_sectors,
};

// ---------------------------------------------------------------------------
// Device: real SDHI1 hardware driver
// ---------------------------------------------------------------------------

/// Whether the card can be used for an operation: an aborted transfer (the controller stopped and
/// reset after a dropped command or transfer, `aborted`) makes it unusable until `init` runs again,
/// as does a card `init` hasn't readied.
#[cfg_attr(not(any(target_os = "none", test)), allow(dead_code))]
fn card_usable(aborted: bool, ready: &core::sync::atomic::AtomicBool) -> Result<(), SdError> {
    use core::sync::atomic::Ordering;
    if aborted {
        ready.store(false, Ordering::Release);
    }
    if ready.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(SdError::NotInitialized)
    }
}

#[cfg(target_os = "none")]
mod device {
    use core::cell::Cell;
    use core::sync::atomic::{AtomicBool, Ordering};

    use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::mutex::Mutex;
    use embassy_sync::once_lock::OnceLock;
    use rza1_hal::Peri;
    use rza1_hal::UNCACHED_MIRROR_OFFSET;
    use rza1_hal::cache;
    use rza1_hal::dma_buf::UncachedMut;
    use rza1_hal::dmac::{self, Dma};
    use rza1_hal::sdhi::card::{self, Card};
    use rza1_hal::sdhi::{self, Sdhi, WpPolarity};

    use super::SdError;
    use crate::board::{SdCard, SdPins, SdRxDma, SdTxDma};
    use crate::system::SD_DMA_MAX_SECTORS;

    rza1_hal::bind_interrupts!(struct Irqs {
        SDHI1_STATUS => sdhi::StatusInterruptHandler<SdCard>;
        SDHI1_OP => sdhi::OperationInterruptHandler<SdCard>;
        DMAINT14 => dmac::InterruptHandler<SdRxDma>;
    });

    /// The SD host controller (SDHI1, the Deluge's slot), once [`init_hw`] has run.
    static SDHI: OnceLock<Sdhi<'static>> = OnceLock::new();

    /// Held across each card operation ([`init`] and the sector reads and writes): the card
    /// protocol runs one command and transfer at a time, through one DMA bounce buffer, so an
    /// operation on a preempting executor waits here instead of interleaving its commands.
    static CARD: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

    /// Bring the SD slot's hardware up, once: pins, SDHI1 with its DMA channels and interrupts.
    /// [`init`] then identifies the card, and runs again after a card change.
    pub fn init_hw(
        card: Peri<'static, SdCard>,
        pins: SdPins,
        tx: Peri<'static, SdTxDma>,
        rx: Peri<'static, SdRxDma>,
    ) {
        let sdhi = Sdhi::new(
            card,
            pins.clk,
            pins.cmd,
            pins.d0,
            pins.d1,
            pins.d2,
            pins.d3,
            pins.cd,
            pins.wp,
            Dma::new_without_interrupt(tx),
            Dma::new(rx, Irqs),
            Irqs,
            crate::system::SD_OPTION,
        );
        // The Deluge's socket leaves SD_WP high when the lock tab is engaged (INFO7 = 0), the
        // opposite of the usual wiring; checked on hardware with locked and unlocked cards.
        sdhi.set_write_protect_polarity(WpPolarity::ActiveHigh);
        if SDHI.init(sdhi).is_err() {
            unreachable!("SDHI1 is owned once");
        }
    }

    /// Whether [`init_hw`] has run: the slot's controller is up, whoever brought it up.
    pub fn hw_ready() -> bool {
        SDHI.try_get().is_some()
    }

    /// The SD host controller, for code that drives it directly (clock experiments).
    ///
    /// # Panics
    /// Before [`init_hw`].
    pub fn controller() -> &'static Sdhi<'static> {
        SDHI.try_get().expect("deluge_bsp::sd::init_hw hasn't run")
    }

    /// Whether a command or transfer was dropped mid-flight since the last check (false before
    /// `init_hw`, when there is no controller to have aborted).
    fn aborted() -> bool {
        SDHI.try_get().is_some_and(|c| c.take_aborted())
    }

    // ---------------------------------------------------------------------------
    // Global card state
    // ---------------------------------------------------------------------------

    /// The card [`init`] identified (its RCA, capacity class and speed), until [`invalidate`].
    static CARD_INFO: BlockingMutex<CriticalSectionRawMutex, Cell<Option<Card>>> =
        BlockingMutex::new(Cell::new(None));
    /// `true` once `init()` has completed successfully.
    static CARD_READY: AtomicBool = AtomicBool::new(false);

    fn card_info() -> Option<Card> {
        CARD_INFO.lock(|c| c.get())
    }

    fn set_card_info(card: Option<Card>) {
        CARD_INFO.lock(|c| c.set(card));
    }

    // ---------------------------------------------------------------------------
    // DMA bounce buffer
    // ---------------------------------------------------------------------------
    //
    // DMA requires uncached memory.  Callers pass arbitrary (possibly cached)
    // buffers, so we maintain a statically-allocated bounce buffer and access it
    // through its uncached alias (physical address | 0x4000_0000).
    //
    // Access is serialised by the single-task async design: only one SD operation
    // runs at a time, so no locking is needed.

    #[repr(align(32))]
    struct SdDmaBuf([u8; SD_DMA_MAX_SECTORS * 512]);
    // Safety: access is serialised by the async executor (single SD task).
    static mut SD_DMA_BUF: SdDmaBuf = SdDmaBuf([0u8; SD_DMA_MAX_SECTORS * 512]);

    /// The bounce buffer, through its uncached mirror, as the DMA buffer it is.
    ///
    /// # Safety
    /// The caller holds [`CARD`] (one SD operation at a time), so no other view of the buffer is
    /// live; the buffer is only ever touched through this mirror view, and `init` cleans and
    /// invalidates its cached alias once.
    unsafe fn bounce() -> UncachedMut<'static> {
        // SAFETY: the caller's contract.
        let mirror = unsafe {
            core::slice::from_raw_parts_mut(
                (core::ptr::addr_of_mut!(SD_DMA_BUF.0[0]) as usize + UNCACHED_MIRROR_OFFSET)
                    as *mut u8,
                SD_DMA_MAX_SECTORS * 512,
            )
        };
        UncachedMut::new(mirror).expect("the SD bounce buffer's mirror is uncached memory")
    }

    // ---------------------------------------------------------------------------
    // Public API
    // ---------------------------------------------------------------------------

    /// Identify the card and bring it to the transfer state (the card protocol).
    ///
    /// Call after [`init_hw`]; call again after a card-swap event.
    ///
    /// # Returns
    /// `Ok(())` on success; `Err(SdError)` on any failure.
    pub async fn init() -> Result<(), SdError> {
        let _card = CARD.lock().await;
        init_unlocked().await
    }

    async fn init_unlocked() -> Result<(), SdError> {
        log::debug!("sd: init");
        CARD_READY.store(false, Ordering::Release);
        // Forget the previous card before re-identifying: if this (re-)init fails, is_hc() and
        // is_hs() must not report the last session's card. (Identification itself always
        // addresses RCA 0, whatever card was here before.)
        set_card_info(None);

        // The pins and interrupts came up once, in `init_hw`. The controller goes back to its
        // identification state each time: the previous card may have left SD_CLK at 16.7 or
        // 33.3 MHz (identification allows 400 kHz), or the controller wedged by a pull
        // mid-transfer.
        // Re-identifying is what an aborted transfer asks for: drop its flag.
        let _ = controller().take_aborted();
        controller().reset();
        unsafe {
            // Clean-invalidate the DMA bounce buffer's cacheable alias.  BSS
            // zeroing wrote dirty lines there; if those lines were later evicted
            // after a DMA fill via the uncached mirror, they would corrupt the
            // received data.
            let buf_start = core::ptr::addr_of!(SD_DMA_BUF) as usize;
            let buf_end = buf_start + core::mem::size_of::<SdDmaBuf>();
            cache::dma_clean_inv_range(buf_start, buf_end);
        }

        // ---- Wait for the card to wake up, then run the protocol ----
        //
        // On a *cold* power-up the card only starts its internal power-on once the
        // pins are muxed and the clock starts (above).  Critically, the SDHI
        // card-detect status also only reports "present" after `Ncycle` SD_CLK
        // cycles elapse with SD_CD held low (SD_OPTION[3:0]); until the card has
        // woken up it won't answer, so CMD55 returns ResponseTimeout (or the OCR
        // busy bit never clears → UnsupportedCard).  The stock Renesas driver simply
        // blind-waits ~1 s here.  Instead we retry the protocol over a ~1 s budget
        // and return the instant it succeeds: a *warm* card (already awake from a
        // previous session) is ready on the first try, while a *cold* card is given
        // the full second it needs.
        const INIT_ATTEMPTS: u32 = 16;
        const RETRY_DELAY_MS: u64 = 75; // 16 × ~(protocol + 75 ms) ≈ 1.3 s budget

        // Minimum supply/clock settle (SD spec: ≥1 ms + 74 clocks) before CMD0.
        embassy_time::Timer::after_millis(15).await;

        let mut last_err = SdError::NotInitialized;
        for attempt in 1..=INIT_ATTEMPTS {
            match card::identify(controller()).await {
                Ok(card) => {
                    set_card_info(Some(card));
                    CARD_READY.store(true, Ordering::Release);
                    // Forget the insertion edges recorded up to here: they announced
                    // the card just identified, so reporting them onward would ask a
                    // caller to redo this work. A removal stays recorded: a card
                    // pulled while this ran is reported by the next
                    // `take_card_detect_events`. `init_hw` ungated the SDHI module
                    // clock, so the INFO1 access is valid here.
                    controller().clear_insertion_events();
                    log::debug!(
                        "sd: card ready on attempt {}/{} (HC={})",
                        attempt,
                        INIT_ATTEMPTS,
                        card.is_high_capacity()
                    );
                    return Ok(());
                }
                Err(e) => {
                    let e = SdError::from(e);
                    last_err = e;
                    log::debug!("sd: init attempt {}/{}: {:?}", attempt, INIT_ATTEMPTS, e);
                    if attempt < INIT_ATTEMPTS {
                        embassy_time::Timer::after_millis(RETRY_DELAY_MS).await;
                    }
                }
            }
        }
        log::warn!(
            "sd: init failed after {} attempts: {:?}",
            INIT_ATTEMPTS,
            last_err
        );
        Err(last_err)
    }

    /// Discard everything cached about the card, so no transfer is attempted until
    /// [`init`] has run again.
    ///
    /// Call this the moment a removal is detected. Every cached value here — the
    /// RCA assigned by CMD3, the SDHC/high-speed flags, the CSD-derived capacity —
    /// describes *that* card, and none of it carries over to whatever is inserted
    /// next. Left in place, the driver keeps addressing the old card's RCA: the new
    /// card never answers, so reads fail (and, because the SDHI command waits are
    /// interrupt-driven with no deadline of their own, a transfer that draws neither
    /// a completion nor an error interrupt does not fail but simply never returns).
    /// Clearing `CARD_READY` makes [`read_sectors`]/[`write_sectors`] refuse with
    /// [`SdError::NotInitialized`] instead, so the failure is immediate and visible.
    ///
    /// [`is_ready`] reads false afterwards, which is also what tells a caller
    /// polling for the card to come back that it has not come back *yet*.
    pub fn invalidate() {
        CARD_READY.store(false, Ordering::Release);
        set_card_info(None);
        // total_sectors() must not answer with the old card's capacity either.
        controller().set_card_blocks(0);
    }

    /// Consume the latched card insert/remove edges: `(removed, inserted)`.
    ///
    /// Edges, not levels — see `rza1_hal::sdhi` for why that
    /// distinction matters (a swap between two polls is invisible to a level read).
    /// Both `true` means the card was replaced.
    pub fn take_card_detect_events() -> (bool, bool) {
        // SAFETY: reads/clears SD_INFO1's card-detect latch for the port this
        // module owns; the clear is written so no other status flag is disturbed.
        controller().take_card_detect_events()
    }

    /// Read a single 512-byte sector at logical block address `lba`.
    ///
    /// # Arguments
    /// * `lba`  — sector number (0-based).
    /// * `buf`  — destination buffer; must be exactly 512 bytes.
    pub async fn read_sector(lba: u32, buf: &mut [u8; 512]) -> Result<(), SdError> {
        let _card = CARD.lock().await;
        read_sector_unlocked(lba, buf).await
    }

    async fn read_sector_unlocked(lba: u32, buf: &mut [u8; 512]) -> Result<(), SdError> {
        read_sectors_unlocked(lba, 1, buf).await
    }

    /// Write a single 512-byte sector at logical block address `lba`.
    ///
    /// # Arguments
    /// * `lba`  — sector number (0-based).
    /// * `buf`  — source data; must be exactly 512 bytes.
    pub async fn write_sector(lba: u32, buf: &[u8; 512]) -> Result<(), SdError> {
        let _card = CARD.lock().await;
        write_sector_unlocked(lba, buf).await
    }

    async fn write_sector_unlocked(lba: u32, buf: &[u8; 512]) -> Result<(), SdError> {
        write_sectors_unlocked(lba, 1, buf).await
    }

    /// Read `count` consecutive sectors starting at `lba`.
    ///
    /// Uses CMD18 (READ_MULTIPLE_BLOCK) when `count > 1`, CMD17 otherwise.
    ///
    /// # Arguments
    /// * `lba`   — first sector (0-based).
    /// * `count` — number of sectors.
    /// * `buf`   — destination; must hold exactly `count * 512` bytes.
    pub async fn read_sectors(lba: u32, count: u32, buf: &mut [u8]) -> Result<(), SdError> {
        let _card = CARD.lock().await;
        read_sectors_unlocked(lba, count, buf).await
    }

    async fn read_sectors_unlocked(lba: u32, count: u32, buf: &mut [u8]) -> Result<(), SdError> {
        super::card_usable(aborted(), &CARD_READY)?;
        let card = card_info().ok_or(SdError::NotInitialized)?;
        if count == 0 {
            return Ok(());
        }
        // SAFETY: the caller holds CARD.
        let mut dma = unsafe { bounce() };
        card::read_blocks(controller(), &card, lba, count, buf, &mut dma).await?;
        Ok(())
    }

    /// Write `count` consecutive sectors starting at `lba`.
    ///
    /// Uses CMD25 (WRITE_MULTIPLE_BLOCK) when `count > 1`, CMD24 otherwise.
    pub async fn write_sectors(lba: u32, count: u32, buf: &[u8]) -> Result<(), SdError> {
        let _card = CARD.lock().await;
        write_sectors_unlocked(lba, count, buf).await
    }

    async fn write_sectors_unlocked(lba: u32, count: u32, buf: &[u8]) -> Result<(), SdError> {
        super::card_usable(aborted(), &CARD_READY)?;
        let card = card_info().ok_or(SdError::NotInitialized)?;
        if count == 0 {
            return Ok(());
        }
        // SAFETY: the caller holds CARD.
        let mut dma = unsafe { bounce() };
        card::write_blocks(controller(), &card, lba, count, buf, &mut dma).await?;
        Ok(())
    }

    /// Returns `true` if `init()` completed successfully and the card is ready.
    pub fn is_ready() -> bool {
        // An aborted transfer makes the card unready until `init` (USB mass storage's
        // `ensure_ready` relies on this to re-initialise).
        super::card_usable(aborted(), &CARD_READY).is_ok()
    }

    /// Returns `true` if the card is a High Capacity (SDHC/SDXC) card.
    ///
    /// Only valid after [`init()`] has returned `Ok(())`.
    pub fn is_hc() -> bool {
        card_info().is_some_and(|c| c.is_high_capacity())
    }

    /// Return `true` if the card is running in High-Speed mode (33.3 MHz SD_CLK).
    pub fn is_hs() -> bool {
        card_info().is_some_and(|c| c.is_high_speed())
    }

    /// Returns `true` if a card is physically present (CD pin).
    pub fn is_inserted() -> bool {
        controller().card_inserted()
    }

    /// Returns `true` if the card's physical write-protect (lock) tab is engaged.
    ///
    /// Reads the SDHI SD_WP signal.  Only meaningful once the controller is up
    /// (after [`init`]); returns `false` when no card is ready.
    pub fn is_write_protected() -> bool {
        if super::card_usable(aborted(), &CARD_READY).is_err() {
            return false;
        }
        controller().card_write_protected()
    }

    /// Total number of 512-byte sectors on the card.
    ///
    /// Only valid after [`init()`] has returned `Ok(())`.  Returns 0 if the card
    /// is not ready.  Used to answer SCSI READ CAPACITY for USB mass storage.
    pub fn total_sectors() -> u32 {
        if super::card_usable(aborted(), &CARD_READY).is_err() {
            return 0;
        }
        controller().card_size_blocks()
    }

    // ---------------------------------------------------------------------------
    // embedded-sdmmc BlockDevice + TimeSource
    // ---------------------------------------------------------------------------

    /// A synchronous `embedded_sdmmc::BlockDevice` backed by the SDHI hardware.
    ///
    /// Uses polling register reads (no Embassy executor needed).  [`init`] must
    /// have completed successfully before any method is called.
    ///
    /// Construct with `DelugeBlockDevice` (it is a ZST).
    #[cfg(feature = "fat")]
    pub struct DelugeBlockDevice;

    #[cfg(feature = "fat")]
    impl embedded_sdmmc::BlockDevice for DelugeBlockDevice {
        type Error = SdError;

        fn read(
            &self,
            blocks: &mut [embedded_sdmmc::Block],
            start_block_idx: embedded_sdmmc::BlockIdx,
        ) -> Result<(), SdError> {
            super::card_usable(aborted(), &CARD_READY)?;
            let count = blocks.len() as u32;
            if count == 0 {
                return Ok(());
            }
            let card = card_info().ok_or(SdError::NotInitialized)?;
            // SAFETY: an embedded-sdmmc `Block` is its 512 content bytes, so the blocks are
            // `count * 512` contiguous bytes, borrowed for this call.
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(blocks.as_mut_ptr() as *mut u8, blocks.len() * 512)
            };
            // Interrupt-driven PIO. embedded-sdmmc's `BlockDevice` is synchronous, so run the
            // async transfer to completion with block_on (fine for the bootloader: nothing else
            // runs). The firmware uses the async DMA path, [`read_sectors`], so the executor keeps
            // running during transfers.
            embassy_futures::block_on(card::read_blocks_pio(
                controller(),
                &card,
                start_block_idx.0,
                count,
                bytes,
            ))
            .map_err(SdError::from)
        }

        fn write(
            &self,
            blocks: &[embedded_sdmmc::Block],
            start_block_idx: embedded_sdmmc::BlockIdx,
        ) -> Result<(), SdError> {
            super::card_usable(aborted(), &CARD_READY)?;
            let count = blocks.len() as u32;
            if count == 0 {
                return Ok(());
            }
            let card = card_info().ok_or(SdError::NotInitialized)?;
            // SAFETY: as in `read`: the blocks are `count * 512` contiguous bytes.
            let bytes = unsafe {
                core::slice::from_raw_parts(blocks.as_ptr() as *const u8, blocks.len() * 512)
            };
            // Interrupt-driven PIO (see `read` above).
            embassy_futures::block_on(card::write_blocks_pio(
                controller(),
                &card,
                start_block_idx.0,
                count,
                bytes,
            ))
            .map_err(SdError::from)
        }

        fn num_blocks(&self) -> Result<embedded_sdmmc::BlockCount, SdError> {
            super::card_usable(aborted(), &CARD_READY)?;
            let n = controller().card_size_blocks();
            Ok(embedded_sdmmc::BlockCount(n))
        }
    }

    // ---------------------------------------------------------------------------
    // Superfloppy (no-MBR) compatibility shim
    // ---------------------------------------------------------------------------

    /// Internal mode for [`PartitionShim`], decided once at construction time.
    #[derive(Clone, Copy)]
    #[cfg(feature = "fat")]
    enum ShimMode {
        /// The card has a real MBR (or we couldn't probe it): forward every
        /// request to [`DelugeBlockDevice`] unchanged.
        Passthrough,
        /// The card is a "superfloppy" — a FAT volume boot record sits directly at
        /// LBA 0 with no partition table. We present a one-block-shifted virtual
        /// address space: virtual LBA 0 returns a synthesized MBR, and virtual LBA
        /// `n` (n >= 1) maps to physical LBA `n - 1`. `total` is the physical card
        /// size in blocks.
        Superfloppy { total: u32 },
    }

    /// A [`BlockDevice`](embedded_sdmmc::BlockDevice) wrapper that transparently
    /// supports both MBR-partitioned and "superfloppy" (no-partition-table) SD
    /// cards.
    ///
    /// [`embedded_sdmmc`] only understands MBR-partitioned cards: it reads LBA 0,
    /// requires the `0x55AA` signature, then validates byte 446 as a partition
    /// status byte. A superfloppy card has a FAT VBR at LBA 0 — which also ends in
    /// `0x55AA` — so the signature check passes but the "partition status" check
    /// reads FAT boot code and fails with `FormatError("Invalid partition status")`.
    ///
    /// This shim detects that case at construction (by probing LBA 0) and, for
    /// superfloppy cards, synthesizes a single-partition MBR pointing at the real
    /// VBR. MBR-partitioned cards are passed straight through with no shift.
    ///
    /// [`init`] must have completed successfully before this is constructed.
    #[cfg(feature = "fat")]
    pub struct PartitionShim {
        inner: DelugeBlockDevice,
        mode: ShimMode,
    }

    #[cfg(feature = "fat")]
    impl PartitionShim {
        /// Probe LBA 0 and pick a [`ShimMode`]. Any read error or ambiguous layout
        /// falls back to [`ShimMode::Passthrough`].
        pub fn new() -> Self {
            let inner = DelugeBlockDevice;
            let mode = Self::detect(&inner);
            Self { inner, mode }
        }

        fn detect(inner: &DelugeBlockDevice) -> ShimMode {
            use embedded_sdmmc::{BlockDevice, BlockIdx};

            let mut block = [embedded_sdmmc::Block::new()];
            if let Err(e) = inner.read(&mut block, BlockIdx(0)) {
                log::warn!("sd: shim probe read failed: {:?} -> passthrough", e);
                return ShimMode::Passthrough;
            }
            let b = &block[0].contents;

            // No boot signature at all: not FAT and not an MBR — let the normal
            // path surface the error.
            if b[510] != 0x55 || b[511] != 0xAA {
                log::warn!("sd: shim no 0x55AA signature -> passthrough");
                return ShimMode::Passthrough;
            }

            // embedded-sdmmc accepts the card as MBR-partitioned when the
            // partition-0 status byte is 0x00 or 0x80.
            let looks_like_mbr = (b[446] & 0x7F) == 0x00;

            // A FAT VBR begins with a short/near jump (0xEB .. 0x90 or 0xE9) and
            // declares 512 bytes per logical sector in its BPB. To avoid a false
            // positive on an MBR whose boot code happens to start with a jump, also
            // require a couple of always-present FAT BPB invariants: a non-zero
            // reserved-sector count (offset 14) and 1 or 2 FATs (offset 16). These
            // hold for every FAT12/16/32 volume but are vanishingly unlikely to all
            // line up in MBR boot code.
            let jump_ok = b[0] == 0xEB || b[0] == 0xE9;
            let bytes_per_sector = u16::from_le_bytes([b[11], b[12]]);
            let reserved_sectors = u16::from_le_bytes([b[14], b[15]]);
            let num_fats = b[16];
            let looks_like_vbr = jump_ok
                && bytes_per_sector == 512
                && reserved_sectors != 0
                && (num_fats == 1 || num_fats == 2);

            // An unambiguous VBR wins even if byte 446 happens to read as a valid
            // MBR status byte (it falls inside the VBR's boot-code region and can be
            // anything): a real FAT VBR at LBA 0 is always a superfloppy.
            if looks_like_vbr {
                let total = inner.num_blocks().map(|c| c.0).unwrap_or(0);
                log::info!("sd: shim -> superfloppy (synthetic MBR, {} blocks)", total);
                ShimMode::Superfloppy { total }
            } else {
                log::info!(
                    "sd: shim -> passthrough (mbr={} vbr={})",
                    looks_like_mbr,
                    looks_like_vbr
                );
                ShimMode::Passthrough
            }
        }
    }

    #[cfg(feature = "fat")]
    impl Default for PartitionShim {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Fill `buf` with a minimal MBR describing one FAT32 (LBA) partition that
    /// starts at LBA 1 and spans `total` blocks.
    fn synthesize_mbr(buf: &mut [u8; 512], total: u32) {
        buf.fill(0);
        // Partition entry 0 lives at offset 446 and is 16 bytes long.
        let p = &mut buf[446..462];
        p[0] = 0x00; // status: non-bootable (passes embedded-sdmmc's check)
        // p[1..4]  CHS of first sector  — ignored for LBA parsing, leave zero.
        p[4] = 0x0C; // partition type: FAT32 with LBA
        // p[5..8]  CHS of last sector   — ignored, leave zero.
        p[8..12].copy_from_slice(&1u32.to_le_bytes()); // LBA of first sector
        p[12..16].copy_from_slice(&total.to_le_bytes()); // number of sectors
        buf[510] = 0x55;
        buf[511] = 0xAA;
    }

    #[cfg(feature = "fat")]
    impl embedded_sdmmc::BlockDevice for PartitionShim {
        type Error = SdError;

        fn read(
            &self,
            blocks: &mut [embedded_sdmmc::Block],
            start_block_idx: embedded_sdmmc::BlockIdx,
        ) -> Result<(), SdError> {
            match self.mode {
                ShimMode::Passthrough => self.inner.read(blocks, start_block_idx),
                ShimMode::Superfloppy { total } => {
                    let start = start_block_idx.0;
                    if start == 0 {
                        if let Some((first, rest)) = blocks.split_first_mut() {
                            synthesize_mbr(&mut first.contents, total);
                            // Virtual blocks 1.. map to physical 0..
                            if !rest.is_empty() {
                                self.inner.read(rest, embedded_sdmmc::BlockIdx(0))?;
                            }
                        }
                        Ok(())
                    } else {
                        self.inner.read(blocks, embedded_sdmmc::BlockIdx(start - 1))
                    }
                }
            }
        }

        fn write(
            &self,
            blocks: &[embedded_sdmmc::Block],
            start_block_idx: embedded_sdmmc::BlockIdx,
        ) -> Result<(), SdError> {
            match self.mode {
                ShimMode::Passthrough => self.inner.write(blocks, start_block_idx),
                ShimMode::Superfloppy { .. } => {
                    let start = start_block_idx.0;
                    if start == 0 {
                        // The synthetic MBR is virtual-only; never write it back.
                        // Forward any trailing real blocks to physical LBA 0..
                        if let Some((_, rest)) = blocks.split_first()
                            && !rest.is_empty()
                        {
                            self.inner.write(rest, embedded_sdmmc::BlockIdx(0))?;
                        }
                        Ok(())
                    } else {
                        self.inner
                            .write(blocks, embedded_sdmmc::BlockIdx(start - 1))
                    }
                }
            }
        }

        fn num_blocks(&self) -> Result<embedded_sdmmc::BlockCount, SdError> {
            match self.mode {
                ShimMode::Passthrough => self.inner.num_blocks(),
                // One extra block for the synthetic MBR at virtual LBA 0.
                ShimMode::Superfloppy { total } => {
                    Ok(embedded_sdmmc::BlockCount(total.saturating_add(1)))
                }
            }
        }
    }

    /// An [`embedded_sdmmc::TimeSource`] that returns a fixed epoch-zero timestamp
    /// (1970-01-01 00:00:00) for all FAT file operations.
    ///
    /// File modification times will not be recorded accurately. No RTC peripheral
    /// is currently available on this BSP.
    #[cfg(feature = "fat")]
    pub struct DelugeTimeSource;

    #[cfg(feature = "fat")]
    impl embedded_sdmmc::TimeSource for DelugeTimeSource {
        fn get_timestamp(&self) -> embedded_sdmmc::Timestamp {
            embedded_sdmmc::Timestamp {
                year_since_1970: 0,
                zero_indexed_month: 0,
                zero_indexed_day: 0,
                hours: 0,
                minutes: 0,
                seconds: 0,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Host: a disk image standing in for the SD card -- a file, or a sparse
// in-memory disk the embedding binary asks for with `use_memory_disk`
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "none"))]
mod host {
    use std::collections::HashMap;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};

    use super::SdError;

    const SECTOR_SIZE: usize = 512;

    /// A disk held in memory, keeping only the sectors written to it. A
    /// sector never written reads as zeroes, as a freshly formatted file's
    /// holes do, so a multi-gigabyte volume costs what was actually written.
    pub(super) struct SparseDisk {
        sectors: u32,
        written: HashMap<u32, Box<[u8; SECTOR_SIZE]>>,
    }

    impl SparseDisk {
        pub(super) fn new(sectors: u32) -> Self {
            Self {
                sectors,
                written: HashMap::new(),
            }
        }

        fn in_range(&self, lba: u32, count: u32) -> Result<(), SdError> {
            match lba.checked_add(count) {
                Some(end) if end <= self.sectors => Ok(()),
                _ => Err(SdError::Protocol),
            }
        }

        pub(super) fn read(&self, lba: u32, count: u32, buf: &mut [u8]) -> Result<(), SdError> {
            self.in_range(lba, count)?;
            for (i, out) in buf
                .as_chunks_mut::<SECTOR_SIZE>()
                .0
                .iter_mut()
                .take(count as usize)
                .enumerate()
            {
                match self.written.get(&(lba + i as u32)) {
                    Some(sector) => out.copy_from_slice(&sector[..]),
                    None => out.fill(0),
                }
            }
            Ok(())
        }

        pub(super) fn write(&mut self, lba: u32, count: u32, buf: &[u8]) -> Result<(), SdError> {
            self.in_range(lba, count)?;
            for (i, data) in buf
                .as_chunks::<SECTOR_SIZE>()
                .0
                .iter()
                .take(count as usize)
                .enumerate()
            {
                let sector = self
                    .written
                    .entry(lba + i as u32)
                    .or_insert_with(|| Box::new([0u8; SECTOR_SIZE]));
                sector.copy_from_slice(data);
            }
            Ok(())
        }

        /// Write the disk out as a sparse file of its full size: only the
        /// sectors held are written, the rest stay holes.
        pub(super) fn dump(&self, path: &Path) -> std::io::Result<()> {
            let mut file = File::create(path)?;
            file.set_len(self.sectors as u64 * SECTOR_SIZE as u64)?;
            let mut lbas: Vec<u32> = self.written.keys().copied().collect();
            lbas.sort_unstable();
            for lba in lbas {
                file.seek(SeekFrom::Start(lba as u64 * SECTOR_SIZE as u64))?;
                file.write_all(&self.written[&lba][..])?;
            }
            Ok(())
        }
    }

    enum Backing {
        File(File),
        Memory(SparseDisk),
    }

    /// Why [`use_memory_disk`] refused.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MemoryDiskError {
        /// The disk was already opened -- as the backing file, or as a memory
        /// disk of a different size. The choice has to come first.
        AlreadyOpen,
    }

    /// The memory disk's size in sectors, when one was asked for.
    static MEMORY_SECTORS: OnceLock<u32> = OnceLock::new();

    /// Stand the card in with a sparse in-memory disk of `sectors` sectors
    /// instead of the backing file. Must come before anything opens the disk
    /// (`init`, a read or a write); a second call with the same size is a
    /// no-op. Nothing touches the file system: the disk starts unformatted
    /// and all zeroes.
    pub fn use_memory_disk(sectors: u32) -> Result<(), MemoryDiskError> {
        let chosen = *MEMORY_SECTORS.get_or_init(|| sectors);
        if chosen != sectors {
            return Err(MemoryDiskError::AlreadyOpen);
        }
        match DISK.get() {
            Some(disk) if !matches!(disk.lock().unwrap().backing, Backing::Memory(_)) => {
                Err(MemoryDiskError::AlreadyOpen)
            }
            _ => Ok(()),
        }
    }

    /// Write the memory disk out to `path` as a sparse file of its full size,
    /// for inspecting what a run left on the card. An error when the card is
    /// the backing file, or has not been opened.
    pub fn dump_memory_disk(path: &Path) -> std::io::Result<()> {
        let disk = DISK.get().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "the disk was never opened")
        })?;
        match &disk.lock().unwrap().backing {
            Backing::Memory(memory) => memory.dump(path),
            Backing::File(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "the card is the backing file, not a memory disk",
            )),
        }
    }

    /// Default image size: enough for host bring-up/round-trip checks, cheap to
    /// allocate (sparse on any filesystem that supports holes).
    const DEFAULT_SECTORS: u64 = 16 * 1024; // 8 MiB

    struct Disk {
        backing: Backing,
        sectors: u32,
    }

    static DISK: OnceLock<Mutex<Disk>> = OnceLock::new();

    /// Mirrors the device's `CARD_READY`: `false` until [`init`] has opened (and,
    /// if needed, created/sized) the backing file.
    static READY: AtomicBool = AtomicBool::new(false);

    fn image_path() -> PathBuf {
        std::env::var_os("DELUGE_SD_IMAGE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("deluge-bsp-sd.img"))
    }

    fn disk() -> &'static Mutex<Disk> {
        DISK.get_or_init(|| {
            if let Some(&sectors) = MEMORY_SECTORS.get() {
                log::info!("sd(host): memory disk ({sectors} sectors)");
                return Mutex::new(Disk {
                    backing: Backing::Memory(SparseDisk::new(sectors)),
                    sectors,
                });
            }
            let path = image_path();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap_or_else(|e| panic!("sd(host): failed to open backing file {path:?}: {e}"));
            let want_len = DEFAULT_SECTORS * SECTOR_SIZE as u64;
            let len = file
                .metadata()
                .unwrap_or_else(|e| panic!("sd(host): failed to stat backing file {path:?}: {e}"))
                .len();
            if len < want_len {
                file.set_len(want_len).unwrap_or_else(|e| {
                    panic!(
                        "sd(host): failed to size backing file {path:?} to {want_len} bytes: {e}"
                    )
                });
            }
            let sectors = (want_len.max(len) / SECTOR_SIZE as u64) as u32;
            log::info!(
                "sd(host): backing file {} ({sectors} sectors)",
                path.display()
            );
            Mutex::new(Disk {
                backing: Backing::File(file),
                sectors,
            })
        })
    }

    /// Host stand-in for [`init`](super::init): opens (and creates/sizes, if
    /// needed) the backing file. No real hardware to bring up — resolves without
    /// ever suspending.
    pub async fn init() -> Result<(), SdError> {
        let _ = disk();
        READY.store(true, Ordering::Release);
        Ok(())
    }

    /// Host stand-in for [`is_ready`](super::is_ready): `true` once [`init`] has
    /// run.
    pub fn is_ready() -> bool {
        READY.load(Ordering::Acquire)
    }

    /// Host stand-in for [`is_inserted`](super::is_inserted): the backing file
    /// stands in for the card for the whole process lifetime, so it always
    /// reads as present.
    pub fn is_inserted() -> bool {
        true
    }

    /// Host stand-in for [`is_write_protected`](super::is_write_protected): no
    /// write-protect concept for a plain host file.
    pub fn is_write_protected() -> bool {
        false
    }

    /// Host stand-in for [`invalidate`](super::invalidate): drops the ready flag,
    /// so [`is_ready`] reads false until [`init`] runs again (it re-opens the same
    /// backing file, so recovery works). Unreachable in practice — the only caller
    /// acts on a removal event, and [`take_card_detect_events`] never reports one
    /// here — but kept behaviourally faithful rather than a silent no-op.
    pub fn invalidate() {
        READY.store(false, Ordering::Release);
    }

    /// Host stand-in for
    /// [`take_card_detect_events`](super::take_card_detect_events): the backing
    /// file stands in for the card for the whole process lifetime, so it is never
    /// removed and never re-inserted — there is no edge to report.
    pub fn take_card_detect_events() -> (bool, bool) {
        (false, false)
    }

    /// Host stand-in for [`total_sectors`](super::total_sectors): the memory
    /// disk's size, or the backing file's sector count (fixed at creation time).
    pub fn total_sectors() -> u32 {
        disk().lock().unwrap().sectors
    }

    /// Host stand-in for [`read_sectors`](super::read_sectors): `count` sectors
    /// from the memory disk or the backing file. Resolves without ever suspending.
    pub async fn read_sectors(lba: u32, count: u32, buf: &mut [u8]) -> Result<(), SdError> {
        if buf.len() < (count as usize) * SECTOR_SIZE {
            return Err(SdError::Protocol);
        }
        if count == 0 {
            return Ok(());
        }
        let mut d = disk().lock().unwrap();
        match &mut d.backing {
            Backing::Memory(memory) => memory.read(lba, count, buf),
            Backing::File(file) => {
                file.seek(SeekFrom::Start(lba as u64 * SECTOR_SIZE as u64))
                    .map_err(|e| SdError::HostIo(e.kind()))?;
                file.read_exact(&mut buf[..(count as usize) * SECTOR_SIZE])
                    .map_err(|e| SdError::HostIo(e.kind()))
            }
        }
    }

    /// Host stand-in for [`write_sectors`](super::write_sectors): `count` sectors
    /// to the memory disk or the backing file. Resolves without ever suspending.
    pub async fn write_sectors(lba: u32, count: u32, buf: &[u8]) -> Result<(), SdError> {
        if buf.len() < (count as usize) * SECTOR_SIZE {
            return Err(SdError::Protocol);
        }
        if count == 0 {
            return Ok(());
        }
        let mut d = disk().lock().unwrap();
        match &mut d.backing {
            Backing::Memory(memory) => memory.write(lba, count, buf),
            Backing::File(file) => {
                file.seek(SeekFrom::Start(lba as u64 * SECTOR_SIZE as u64))
                    .map_err(|e| SdError::HostIo(e.kind()))?;
                file.write_all(&buf[..(count as usize) * SECTOR_SIZE])
                    .map_err(|e| SdError::HostIo(e.kind()))
            }
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    #[test]
    fn an_aborted_transfer_requires_reinit() {
        use core::sync::atomic::{AtomicBool, Ordering};
        let ready = AtomicBool::new(true);
        assert_eq!(
            super::card_usable(true, &ready),
            Err(super::SdError::NotInitialized)
        );
        assert!(!ready.load(Ordering::Relaxed));
        assert_eq!(
            super::card_usable(false, &ready),
            Err(super::SdError::NotInitialized),
            "still, until init"
        );
        ready.store(true, Ordering::Relaxed);
        assert_eq!(super::card_usable(false, &ready), Ok(()));
    }

    use super::*;

    #[test]
    fn init_reports_ready() {
        embassy_futures::block_on(init()).expect("host init never fails");
        assert!(is_ready());
    }

    #[test]
    fn host_reports_always_inserted_and_not_write_protected() {
        assert!(is_inserted());
        assert!(!is_write_protected());
    }

    /// The host backing file is never removed, so there is never a card-detect
    /// edge to report — and, since a `(removed, _)` report is what drives
    /// `invalidate()`, that is also why nothing on host ever invalidates the
    /// card. (`invalidate()` itself is deliberately untested here: it clears the
    /// shared `READY` flag the other tests in this module rely on, and these run
    /// concurrently.)
    #[test]
    fn host_never_reports_a_card_detect_edge() {
        assert_eq!(take_card_detect_events(), (false, false));
        // Idempotent: repeated draining does not manufacture an edge either.
        assert_eq!(take_card_detect_events(), (false, false));
    }

    #[test]
    fn total_sectors_matches_default_image_size() {
        embassy_futures::block_on(init()).expect("host init never fails");
        assert_eq!(total_sectors(), 16 * 1024);
    }

    // The sparse memory disk, tested as a value: the module's own disk is one
    // process-wide static the tests above share as a file.

    #[test]
    fn sparse_disk_reads_zeroes_where_nothing_was_written() {
        let disk = host::SparseDisk::new(64);
        let mut buf = [0xAAu8; 1024];
        disk.read(10, 2, &mut buf).expect("in range");
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn sparse_disk_round_trips_and_keeps_only_what_was_written() {
        let mut disk = host::SparseDisk::new(64);
        let data: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
        disk.write(7, 2, &data).expect("in range");
        let mut back = [0u8; 1536];
        disk.read(6, 3, &mut back).expect("in range");
        assert!(back[..512].iter().all(|&b| b == 0));
        assert_eq!(&back[512..], &data[..]);
    }

    #[test]
    fn sparse_disk_refuses_out_of_range() {
        let mut disk = host::SparseDisk::new(8);
        let mut buf = [0u8; 1024];
        assert!(disk.read(7, 2, &mut buf).is_err());
        assert!(disk.write(u32::MAX, 1, &buf).is_err());
    }

    #[test]
    fn sparse_disk_dumps_a_full_size_file_with_its_sectors_in_place() {
        let mut disk = host::SparseDisk::new(32);
        disk.write(20, 1, &[0x5Au8; 512]).expect("in range");
        let path = std::env::temp_dir().join(format!("sparse-dump-{}.img", std::process::id()));
        disk.dump(&path).expect("dump");
        let bytes = std::fs::read(&path).expect("read back");
        std::fs::remove_file(&path).ok();
        assert_eq!(bytes.len(), 32 * 512);
        assert!(bytes[20 * 512..21 * 512].iter().all(|&b| b == 0x5A));
        assert!(bytes[..20 * 512].iter().all(|&b| b == 0));
    }

    /// Round-trip a sector of non-trivial data through the host disk image.
    /// Uses a sector index (1000) not touched by the other tests in this module
    /// so they can run concurrently against the shared backing file.
    #[test]
    fn round_trip_sector() {
        embassy_futures::block_on(init()).expect("host init never fails");

        let sector: u32 = 1000;
        let mut pattern = [0u8; 512];
        for (i, b) in pattern.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }

        embassy_futures::block_on(write_sectors(sector, 1, &pattern)).expect("write_sectors");

        let mut readback = [0u8; 512];
        embassy_futures::block_on(read_sectors(sector, 1, &mut readback)).expect("read_sectors");

        assert_eq!(pattern, readback);
    }

    /// Multi-sector round trip, at an offset that doesn't overlap
    /// [`round_trip_sector`]'s range.
    #[test]
    fn round_trip_multiple_sectors() {
        embassy_futures::block_on(init()).expect("host init never fails");

        let sector: u32 = 2000;
        let count: u32 = 4;
        let mut pattern = vec![0u8; count as usize * 512];
        for (i, b) in pattern.iter_mut().enumerate() {
            *b = ((i * 7) % 256) as u8;
        }

        embassy_futures::block_on(write_sectors(sector, count, &pattern)).expect("write_sectors");

        let mut readback = vec![0u8; count as usize * 512];
        embassy_futures::block_on(read_sectors(sector, count, &mut readback))
            .expect("read_sectors");

        assert_eq!(pattern, readback);
    }

    #[test]
    fn read_sectors_rejects_undersized_buffer() {
        let mut too_small = [0u8; 511];
        let err = embassy_futures::block_on(read_sectors(0, 1, &mut too_small))
            .expect_err("buffer is one byte short of a sector");
        assert_eq!(err, SdError::Protocol);
    }
}

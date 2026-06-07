//! USB Mass Storage Bulk-Only Transport (BOT) + SCSI command loop, generic over
//! a backing block device.
//!
//! The three-phase BOT flow per command is:
//!
//! ```text
//! CBW (31 B, bulk OUT) → [optional data phase] → CSW (13 B, bulk IN)
//! ```
//!
//! Only the SCSI commands a host needs to mount and read/write a FAT volume are
//! implemented; unknown opcodes are failed with ILLEGAL REQUEST sense.
//!
//! The storage medium is abstracted by [`BlockDevice`], so the same protocol
//! code drives both the SD-card MSC firmware and the SSB's synthesized UF2
//! "ghost" filesystem.  [`run`] owns the bulk endpoints and the bounce buffers
//! and overlaps the storage transfer with the USB transfer (one bank streams
//! over USB while the other is filled/drained by the device).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use log::{error, info, warn};

use super::{Rusb1EndpointIn, Rusb1EndpointOut};
use embassy_usb_driver::{Endpoint, EndpointIn, EndpointOut};

use crate::usb::classes::msc::take_reset;

// ── Throughput counters (read by an OLED/status task) ──────────────────────────

/// Total bytes sent to the host (SCSI READ / device→host).
pub static TX_BYTES: AtomicU64 = AtomicU64::new(0);
/// Total bytes received from the host (SCSI WRITE / host→device).
pub static RX_BYTES: AtomicU64 = AtomicU64::new(0);

// ── BOT / SCSI constants ──────────────────────────────────────────────────────

const CBW_SIGNATURE: u32 = 0x4342_5355; // "USBC"
const CSW_SIGNATURE: u32 = 0x5342_5355; // "USBS"
const CBW_LEN: usize = 31;
const CSW_LEN: usize = 13;

const CSW_STATUS_GOOD: u8 = 0x00;
const CSW_STATUS_FAILED: u8 = 0x01;

/// SCSI logical block size.  Both backends use 512-byte blocks.
pub const BLOCK_SIZE: u32 = 512;

/// Per-iteration transfer chunk in sectors.  64 × 512 = 32 KiB, which stays
/// below the 65535-byte endpoint transfer-length limit while amortising setup.
const CHUNK_SECTORS: u32 = 64;
const BUF_BYTES: usize = CHUNK_SECTORS as usize * BLOCK_SIZE as usize;

/// Double bounce buffer for the data phase.  Only the single BOT task touches it
/// and the executor is single-threaded, so the `static mut` access is sound.
/// One instance is shared by all firmwares (only one BOT loop runs at a time).
static mut MSC_BUF: [[u8; BUF_BYTES]; 2] = [[0; BUF_BYTES]; 2];

// ── SCSI sense ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Sense {
    key: u8,
    asc: u8,
    ascq: u8,
}

impl Sense {
    const GOOD: Sense = Sense {
        key: 0x00,
        asc: 0x00,
        ascq: 0x00,
    };
    const NOT_READY: Sense = Sense {
        key: 0x02,
        asc: 0x3A,
        ascq: 0x00,
    }; // medium not present
    const MEDIUM_ERROR: Sense = Sense {
        key: 0x03,
        asc: 0x11,
        ascq: 0x00,
    }; // unrecovered error
    const INVALID_COMMAND: Sense = Sense {
        key: 0x05,
        asc: 0x20,
        ascq: 0x00,
    }; // invalid opcode
    const DATA_PROTECT: Sense = Sense {
        key: 0x07,
        asc: 0x27,
        ascq: 0x00,
    }; // write protected
}

// ── Block-device abstraction ──────────────────────────────────────────────────

/// SCSI INQUIRY identity strings for the exposed logical unit.
pub struct Inquiry {
    /// Vendor identification (8 ASCII bytes, space-padded).
    pub vendor: [u8; 8],
    /// Product identification (16 ASCII bytes, space-padded).
    pub product: [u8; 16],
    /// Product revision level (4 ASCII bytes).
    pub revision: [u8; 4],
}

/// A 512-byte-block storage medium that the BOT loop bridges to USB.
///
/// `read`/`write` operate on sector *ranges*; [`run`] supplies disjoint bounce
/// buffers so implementations that can overlap I/O (e.g. SD DMA) do so naturally
/// when joined against the USB transfer.
#[allow(async_fn_in_trait)]
pub trait BlockDevice {
    /// Total number of addressable 512-byte blocks.
    fn block_count(&self) -> u32;
    /// Whether the medium is currently present/ready.
    fn is_ready(&self) -> bool;
    /// Attempt to (re)initialise the medium; returns the resulting readiness.
    async fn ensure_ready(&mut self) -> bool;
    /// Read `count` blocks starting at `lba` into `buf` (`count * 512` bytes).
    async fn read(&mut self, lba: u32, count: u32, buf: &mut [u8]) -> Result<(), ()>;
    /// Write `count` blocks starting at `lba` from `buf` (`count * 512` bytes).
    async fn write(&mut self, lba: u32, count: u32, buf: &[u8]) -> Result<(), ()>;
    /// INQUIRY identity for this unit.
    fn inquiry(&self) -> Inquiry;
    /// Whether the medium is write-protected (e.g. the SD card's lock tab).
    ///
    /// Defaults to writable; backends with a physical/logical WP signal override
    /// this so the BOT layer reports it in MODE SENSE and rejects SCSI WRITE.
    fn is_write_protected(&self) -> bool {
        false
    }
}

// ── Ready-made SD-card backing ────────────────────────────────────────────────

/// [`BlockDevice`] backed by the inserted SD card ([`crate::sd`]).  Shared by the
/// MSC firmware and the bootloader's DATA TRANSFER mode.
pub struct SdBlock;

impl BlockDevice for SdBlock {
    fn block_count(&self) -> u32 {
        crate::sd::total_sectors()
    }

    fn is_ready(&self) -> bool {
        crate::sd::is_ready()
    }

    async fn ensure_ready(&mut self) -> bool {
        if !crate::sd::is_ready() {
            let _ = crate::sd::init().await;
        }
        crate::sd::is_ready()
    }

    async fn read(&mut self, lba: u32, count: u32, buf: &mut [u8]) -> Result<(), ()> {
        crate::sd::read_sectors(lba, count, buf)
            .await
            .map_err(|_| ())
    }

    async fn write(&mut self, lba: u32, count: u32, buf: &[u8]) -> Result<(), ()> {
        crate::sd::write_sectors(lba, count, buf)
            .await
            .map_err(|_| ())
    }

    fn inquiry(&self) -> Inquiry {
        Inquiry {
            vendor: *b"Synthstm",
            product: *b"Deluge SD Card  ",
            revision: *b"1.00",
        }
    }

    fn is_write_protected(&self) -> bool {
        crate::sd::is_write_protected()
    }
}

// ── Transport loop ──────────────────────────────────────────────────────────

/// An exit flag that is never set — used by [`run`] to mean "run forever".
static NEVER_EXIT: AtomicBool = AtomicBool::new(false);

/// Run the Bulk-Only Transport / SCSI command loop against `dev`.  Never returns.
pub async fn run<B: BlockDevice>(dev: B, ep_in: Rusb1EndpointIn, ep_out: Rusb1EndpointOut) -> ! {
    run_until(dev, ep_in, ep_out, &NEVER_EXIT).await;
    // `NEVER_EXIT` is never set, so `run_until` never returns.
    unreachable!()
}

/// Poll an exit flag until it is set.
async fn wait_exit(exit: &AtomicBool) {
    loop {
        if exit.load(Ordering::Acquire) {
            return;
        }
        Timer::after(Duration::from_millis(8)).await;
    }
}

/// Run the BOT / SCSI loop until `exit` is set, then return cleanly.
///
/// Crucially, `exit` is honoured **only between commands** — never in the middle
/// of a data phase.  Cancelling a SCSI WRITE mid-transfer would abandon a
/// multi-block program on the SD card (auto-CMD12 never issued, DMA still armed),
/// which both tears the on-disk FAT structures and leaves the card stuck so the
/// next `sd::init` times out.  So the exit flag races only the CBW read; once a
/// command is accepted it runs to completion uninterrupted.
pub async fn run_until<B: BlockDevice>(
    mut dev: B,
    mut ep_in: Rusb1EndpointIn,
    mut ep_out: Rusb1EndpointOut,
    exit: &AtomicBool,
) {
    // Clear any stale request so a press during the menu doesn't bail out before
    // the first command.
    exit.store(false, Ordering::Release);

    let mut sense = Sense::GOOD;
    let mut cbw = [0u8; 64];

    loop {
        // Discard any pending Bulk-Only Reset before reading the next CBW.
        let _ = take_reset();

        // ── Command phase: read the CBW (the only safe exit point) ───────────
        let n = match select(ep_out.read(&mut cbw), wait_exit(exit)).await {
            Either::First(Ok(n)) => n,
            Either::First(Err(_)) => {
                ep_out.wait_enabled().await;
                continue;
            }
            // Exit requested with no data phase in flight — leave cleanly.
            Either::Second(()) => return,
        };
        if n < CBW_LEN {
            warn!("BOT: short CBW ({} bytes)", n);
            continue;
        }
        let signature = u32::from_le_bytes([cbw[0], cbw[1], cbw[2], cbw[3]]);
        if signature != CBW_SIGNATURE {
            warn!("BOT: bad CBW signature {:08x}", signature);
            continue;
        }
        let tag = u32::from_le_bytes([cbw[4], cbw[5], cbw[6], cbw[7]]);
        let data_len = u32::from_le_bytes([cbw[8], cbw[9], cbw[10], cbw[11]]);
        let dir_in = cbw[12] & 0x80 != 0;
        let cb_len = (cbw[14] & 0x1F) as usize;
        let cdb = &cbw[15..15 + cb_len.min(16)];

        // ── Data phase + status ──────────────────────────────────────────────
        let (status, residue) = handle_command(
            &mut dev,
            cdb,
            data_len,
            dir_in,
            &mut ep_in,
            &mut ep_out,
            &mut sense,
        )
        .await;

        // ── Status phase: send the CSW ───────────────────────────────────────
        if send_csw(&mut ep_in, tag, residue, status).await.is_err() {
            warn!("BOT: CSW write failed");
        }
    }
}

/// Dispatch one SCSI command.  Returns `(csw_status, data_residue)`.
async fn handle_command<B: BlockDevice>(
    dev: &mut B,
    cdb: &[u8],
    data_len: u32,
    dir_in: bool,
    ep_in: &mut Rusb1EndpointIn,
    ep_out: &mut Rusb1EndpointOut,
    sense: &mut Sense,
) -> (u8, u32) {
    if cdb.is_empty() {
        *sense = Sense::INVALID_COMMAND;
        return (CSW_STATUS_FAILED, data_len);
    }

    match cdb[0] {
        // TEST UNIT READY — report medium presence (retry init if needed).
        0x00 => {
            if !dev.is_ready() {
                let _ = dev.ensure_ready().await;
            }
            if dev.is_ready() {
                *sense = Sense::GOOD;
                (CSW_STATUS_GOOD, data_len)
            } else {
                *sense = Sense::NOT_READY;
                (CSW_STATUS_FAILED, data_len)
            }
        }

        // REQUEST SENSE — return fixed-format sense (18 bytes).
        0x03 => {
            let s = *sense;
            let data = [
                0x70, 0x00, s.key, 0x00, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, s.asc,
                s.ascq, 0x00, 0x00, 0x00, 0x00,
            ];
            *sense = Sense::GOOD;
            send_data_in(ep_in, &data, data_len).await
        }

        // INQUIRY — standard 36-byte response.
        0x12 => {
            let id = dev.inquiry();
            let mut data = [0u8; 36];
            data[0] = 0x00; // direct-access block device, connected
            data[1] = 0x80; // RMB = 1 (removable)
            data[2] = 0x04; // SPC-2
            data[3] = 0x02; // response data format = 2
            data[4] = 0x1F; // additional length = 31
            data[8..16].copy_from_slice(&id.vendor);
            data[16..32].copy_from_slice(&id.product);
            data[32..36].copy_from_slice(&id.revision);
            send_data_in(ep_in, &data, data_len).await
        }

        // MODE SENSE(6) — minimal 4-byte header.  Byte 2 bit 7 is the WP flag:
        // set it from the medium's write-protect state (the SD lock tab) so the
        // host mounts the volume read-only.
        0x1A => {
            let wp = if dev.is_write_protected() { 0x80 } else { 0x00 };
            let data = [0x03, 0x00, wp, 0x00];
            send_data_in(ep_in, &data, data_len).await
        }

        // PREVENT/ALLOW MEDIUM REMOVAL, START STOP UNIT, SYNCHRONIZE CACHE(10)
        // — accepted as no-ops.
        0x1E | 0x1B | 0x35 => {
            *sense = Sense::GOOD;
            (CSW_STATUS_GOOD, data_len)
        }

        // READ FORMAT CAPACITIES — current capacity descriptor.
        0x23 => {
            let total = dev.block_count();
            let nb = total.to_be_bytes();
            let bl = BLOCK_SIZE.to_be_bytes();
            let data = [
                0x00, 0x00, 0x00, 0x08, // capacity list header: list length = 8
                nb[0], nb[1], nb[2], nb[3], // number of blocks
                0x02, bl[1], bl[2], bl[3], // descriptor code 2 (formatted) + block len
            ];
            send_data_in(ep_in, &data, data_len).await
        }

        // READ CAPACITY(10) — last LBA + block size, big-endian.
        0x25 => {
            if !dev.is_ready() {
                *sense = Sense::NOT_READY;
                return (CSW_STATUS_FAILED, data_len);
            }
            let last = dev.block_count().saturating_sub(1);
            let mut data = [0u8; 8];
            data[0..4].copy_from_slice(&last.to_be_bytes());
            data[4..8].copy_from_slice(&BLOCK_SIZE.to_be_bytes());
            send_data_in(ep_in, &data, data_len).await
        }

        // READ(10) — stream blocks device→host.
        0x28 => {
            if cdb.len() < 10 {
                *sense = Sense::INVALID_COMMAND;
                return (CSW_STATUS_FAILED, data_len);
            }
            let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]);
            let blocks = u16::from_be_bytes([cdb[7], cdb[8]]) as u32;
            read_blocks(dev, ep_in, lba, blocks, data_len, sense).await
        }

        // WRITE(10) — stream blocks host→device.
        0x2A => {
            if cdb.len() < 10 {
                *sense = Sense::INVALID_COMMAND;
                return (CSW_STATUS_FAILED, data_len);
            }
            // Honour the medium's write-protect (SD lock tab): accept and discard
            // the host's data-out phase so the pipe stays in sync, then fail the
            // command with WRITE PROTECTED sense instead of touching the card.
            if dev.is_write_protected() {
                warn!("BOT: WRITE rejected — medium is write-protected (lock tab)");
                drain_data_out(ep_out, data_len).await;
                *sense = Sense::DATA_PROTECT;
                return (CSW_STATUS_FAILED, data_len);
            }
            let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]);
            let blocks = u16::from_be_bytes([cdb[7], cdb[8]]) as u32;
            write_blocks(dev, ep_out, lba, blocks, data_len, sense).await
        }

        other => {
            warn!("BOT: unsupported SCSI opcode {:#04x}", other);
            // Drain an unexpected host data-out phase so the pipe stays in sync.
            if !dir_in && data_len > 0 {
                drain_data_out(ep_out, data_len).await;
            }
            *sense = Sense::INVALID_COMMAND;
            (CSW_STATUS_FAILED, data_len)
        }
    }
}

/// Read and discard a host data-out phase of `data_len` bytes, keeping the bulk
/// pipe in sync when a command is rejected without committing the data (e.g. an
/// unsupported opcode, or a WRITE to a write-protected medium).
async fn drain_data_out(ep_out: &mut Rusb1EndpointOut, data_len: u32) {
    if data_len == 0 {
        return;
    }
    let buf = unsafe { &mut (*core::ptr::addr_of_mut!(MSC_BUF))[0] };
    let mut remaining = data_len as usize;
    while remaining > 0 {
        let want = remaining.min(BUF_BYTES);
        match ep_out.read(&mut buf[..want]).await {
            Ok(got) if got > 0 => remaining -= got.min(remaining),
            _ => break,
        }
    }
}

/// Send a short data-in response (≤ `data_len` bytes), returning `(status, residue)`.
async fn send_data_in(ep_in: &mut Rusb1EndpointIn, data: &[u8], data_len: u32) -> (u8, u32) {
    let len = (data.len()).min(data_len as usize);
    if len > 0 && ep_in.write(&data[..len]).await.is_err() {
        return (CSW_STATUS_FAILED, data_len);
    }
    TX_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    // Report any Hi > Di shortfall via the CSW residue.  We deliberately do not
    // halt the IN endpoint: the CSW must still go out on it, and all major hosts
    // accept a short data-in phase plus a residue.
    (CSW_STATUS_GOOD, data_len - len as u32)
}

/// Stream `blocks` sectors from the device to the host (SCSI READ).
async fn read_blocks<B: BlockDevice>(
    dev: &mut B,
    ep_in: &mut Rusb1EndpointIn,
    lba: u32,
    blocks: u32,
    data_len: u32,
    sense: &mut Sense,
) -> (u8, u32) {
    let [b0, b1] = unsafe { &mut *core::ptr::addr_of_mut!(MSC_BUF) };
    let mut status = CSW_STATUS_GOOD;
    let mut sent: u32 = 0;
    let t_all = Instant::now();

    let mut remaining = blocks;
    let mut cur_lba = lba;

    // Prime: read the first chunk into bank 0.
    let mut cur_chunk = remaining.min(CHUNK_SECTORS);
    let cb = (cur_chunk * BLOCK_SIZE) as usize;
    if dev.read(cur_lba, cur_chunk, &mut b0[..cb]).await.is_err() {
        error!("BOT: read lba={} failed", cur_lba);
        *sense = Sense::MEDIUM_ERROR;
        return (CSW_STATUS_FAILED, data_len);
    }
    remaining -= cur_chunk;
    cur_lba += cur_chunk;
    let mut cur_in_b0 = true;

    loop {
        let cb = (cur_chunk * BLOCK_SIZE) as usize;
        if remaining > 0 {
            // Overlap: send the current bank over USB while reading the next.
            let next_chunk = remaining.min(CHUNK_SECTORS);
            let nb = (next_chunk * BLOCK_SIZE) as usize;
            let (wr, rd) = if cur_in_b0 {
                join(
                    ep_in.write(&b0[..cb]),
                    dev.read(cur_lba, next_chunk, &mut b1[..nb]),
                )
                .await
            } else {
                join(
                    ep_in.write(&b1[..cb]),
                    dev.read(cur_lba, next_chunk, &mut b0[..nb]),
                )
                .await
            };
            if wr.is_err() {
                status = CSW_STATUS_FAILED;
                break;
            }
            sent += cb as u32;
            TX_BYTES.fetch_add(cb as u64, Ordering::Relaxed);
            if rd.is_err() {
                error!("BOT: read lba={} failed", cur_lba);
                *sense = Sense::MEDIUM_ERROR;
                status = CSW_STATUS_FAILED;
                break;
            }
            cur_chunk = next_chunk;
            cur_lba += next_chunk;
            remaining -= next_chunk;
            cur_in_b0 = !cur_in_b0;
        } else {
            // Last chunk: nothing left to prefetch — just send it.
            let wr = if cur_in_b0 {
                ep_in.write(&b0[..cb]).await
            } else {
                ep_in.write(&b1[..cb]).await
            };
            if wr.is_err() {
                status = CSW_STATUS_FAILED;
                break;
            }
            sent += cb as u32;
            TX_BYTES.fetch_add(cb as u64, Ordering::Relaxed);
            break;
        }
    }
    log_throughput("READ ", sent, t_all.elapsed().as_micros());
    (status, data_len.saturating_sub(sent))
}

/// Stream `blocks` sectors from the host to the device (SCSI WRITE).
async fn write_blocks<B: BlockDevice>(
    dev: &mut B,
    ep_out: &mut Rusb1EndpointOut,
    lba: u32,
    blocks: u32,
    data_len: u32,
    sense: &mut Sense,
) -> (u8, u32) {
    let [b0, b1] = unsafe { &mut *core::ptr::addr_of_mut!(MSC_BUF) };
    let mut status = CSW_STATUS_GOOD;
    let mut recvd: u32 = 0;
    let t_all = Instant::now();

    let mut remaining = blocks;

    // Prime: receive the first chunk from USB into bank 0.
    let mut cur_chunk = remaining.min(CHUNK_SECTORS);
    let cb = (cur_chunk * BLOCK_SIZE) as usize;
    match ep_out.read(&mut b0[..cb]).await {
        Ok(got) if got == cb => {
            recvd += cb as u32;
            RX_BYTES.fetch_add(cb as u64, Ordering::Relaxed);
        }
        _ => return (CSW_STATUS_FAILED, data_len),
    }
    remaining -= cur_chunk;
    let mut cur_in_b0 = true;
    let mut write_lba = lba;

    loop {
        let cb = (cur_chunk * BLOCK_SIZE) as usize;
        if remaining > 0 {
            // Overlap: commit the current bank while receiving the next.
            let next_chunk = remaining.min(CHUNK_SECTORS);
            let nb = (next_chunk * BLOCK_SIZE) as usize;
            let (wr, rd) = if cur_in_b0 {
                join(
                    dev.write(write_lba, cur_chunk, &b0[..cb]),
                    ep_out.read(&mut b1[..nb]),
                )
                .await
            } else {
                join(
                    dev.write(write_lba, cur_chunk, &b1[..cb]),
                    ep_out.read(&mut b0[..nb]),
                )
                .await
            };
            if wr.is_err() {
                error!("BOT: write lba={} failed", write_lba);
                *sense = Sense::MEDIUM_ERROR;
                status = CSW_STATUS_FAILED;
                break;
            }
            write_lba += cur_chunk;
            match rd {
                Ok(got) if got == nb => {
                    recvd += nb as u32;
                    RX_BYTES.fetch_add(nb as u64, Ordering::Relaxed);
                }
                Ok(got) => {
                    warn!("BOT: short WRITE data ({} of {})", got, nb);
                    status = CSW_STATUS_FAILED;
                    break;
                }
                Err(_) => {
                    status = CSW_STATUS_FAILED;
                    break;
                }
            }
            cur_chunk = next_chunk;
            remaining -= next_chunk;
            cur_in_b0 = !cur_in_b0;
        } else {
            // Last chunk: nothing left to receive — just commit it.
            let wr = if cur_in_b0 {
                dev.write(write_lba, cur_chunk, &b0[..cb]).await
            } else {
                dev.write(write_lba, cur_chunk, &b1[..cb]).await
            };
            if wr.is_err() {
                error!("BOT: write lba={} failed", write_lba);
                *sense = Sense::MEDIUM_ERROR;
                status = CSW_STATUS_FAILED;
            }
            break;
        }
    }
    log_throughput("WRITE", recvd, t_all.elapsed().as_micros());
    (status, data_len.saturating_sub(recvd))
}

/// Log effective throughput for large transfers (≥ 64 KiB) over RTT.
/// `bytes / micros` conveniently equals MB/s.
fn log_throughput(op: &str, bytes: u32, total_us: u64) {
    if bytes < 65536 || total_us == 0 {
        return;
    }
    let tenths = bytes as u64 * 10 / total_us;
    info!(
        "BOT {} {}KB in {}us => {}.{} MB/s",
        op,
        bytes / 1024,
        total_us,
        tenths / 10,
        tenths % 10,
    );
}

/// Build and send the 13-byte Command Status Wrapper.
async fn send_csw(
    ep_in: &mut Rusb1EndpointIn,
    tag: u32,
    residue: u32,
    status: u8,
) -> Result<(), ()> {
    let mut csw = [0u8; CSW_LEN];
    csw[0..4].copy_from_slice(&CSW_SIGNATURE.to_le_bytes());
    csw[4..8].copy_from_slice(&tag.to_le_bytes());
    csw[8..12].copy_from_slice(&residue.to_le_bytes());
    csw[12] = status;
    ep_in.write(&csw).await.map_err(|_| ())
}

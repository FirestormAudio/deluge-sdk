//! The DMAC by channel number: what drivers call until Phase 4b migrates them to
//! [`super::Dma`]. Not part of the public API.

use super::*;

/// Initialise a DMA channel in link-descriptor mode.
///
/// Mirrors `initDMAWithLinkDescriptor()` from the C firmware:
/// 1. Clears DCTRL for the channel's group.
/// 2. Loads `CHCFG` from word \[4\] of the descriptor.
/// 3. Programs the DMARS resource selector.
/// 4. Writes the descriptor pointer to `NXLA`.
///
/// # Safety
/// - `descriptor` must point to a valid, 32-byte-aligned 8-word link
///   descriptor whose lifetime extends past the end of the DMA operation.
/// - Must be called before [`channel_start`] for the same channel.
pub(super) unsafe fn init_with_link_descriptor(ch: u8, descriptor: *const u32, dmars_val: u32) {
    unsafe {
        log::trace!(
            "dmac: ch{} init, desc={:#010x}, dmars={:#06x}",
            ch,
            descriptor as usize,
            dmars_val
        );
        // 1. Clear group DCTRL (priority mode / round-robin reset)
        log::trace!("dmac: ch{} writing DCTRL={:#010x}", ch, dctrl_reg(ch).0);
        crate::mmio::write32(dctrl_reg(ch).0, 0);
        log::trace!("dmac: ch{} DCTRL ok", ch);

        // 2. CHCFG comes from link-descriptor word [4]
        let chcfg_val = descriptor.add(4).read();
        crate::mmio::write32(ch_reg(ch, OFF_CHCFG).0, chcfg_val);
        log::trace!("dmac: ch{} CHCFG ok", ch);

        // 3. DMARS: even channel → bits [15:0], odd channel → bits [31:16]
        let dmars = dmars_reg(ch).0;
        let (shifted, mask) = if ch & 1 == 0 {
            (dmars_val & 0xFFFF, 0xFFFF_0000u32)
        } else {
            ((dmars_val & 0xFFFF) << 16, 0x0000_FFFFu32)
        };
        crate::mmio::write32(dmars, (crate::mmio::read32(dmars) & mask) | shifted);
        log::trace!("dmac: ch{} DMARS ok", ch);

        // 4. NXLA = address of the link descriptor
        crate::mmio::write32(ch_reg(ch, OFF_NXLA).0, descriptor as u32);
        log::trace!("dmac: ch{} NXLA ok", ch);
    }
}

/// Start a DMA channel: software-reset then set-enable.
///
/// Mirrors `dmaChannelStart()` from the C firmware.
///
/// A DSB is inserted between SWRST and SETEN to ensure the peripheral sees
/// the reset complete before the enable is asserted (ARMv7-A write-buffer
/// ordering requirement for back-to-back MMIO writes to the same device).
///
/// # Safety
/// The channel must have been initialised with [`init_with_link_descriptor`].
pub(super) unsafe fn channel_start(ch: u8) {
    unsafe {
        log::trace!("dmac: ch{} start (SWRST + SETEN)", ch);
        let chctrl = ch_reg(ch, OFF_CHCTRL);
        // CHCTRL bits are write-1-to-trigger; write as clean values, not RMW.
        chctrl.write(CHCTRL_SWRST);
        // DSB required: drain write buffer so SWRST reaches the DMAC before SETEN.
        // (Host builds — the simulator — have no DMAC and no barrier instruction.)
        #[cfg(target_os = "none")]
        core::arch::asm!("dsb", options(nostack));
        chctrl.write(CHCTRL_SETEN);
    }
}

/// Stop a DMA channel immediately: clear its enable bit, then software-reset
/// it so any in-flight (including circular/peripheral-driven) transfer ceases.
/// Also clears the channel's half of its DMARS register, releasing its
/// peripheral-request route; the paired channel's selector is left alone.
///
/// Intended for use before handing the SoC to another program — a circular
/// channel such as the SCIF RX DMA keeps writing to its buffer forever and is
/// unaffected by masking CPU interrupts, so it must be stopped explicitly or
/// it will corrupt the next program's memory.
///
/// # Safety
/// Writes the channel's `CHCTRL` and DMARS registers.
pub(super) unsafe fn stop(ch: u8) {
    unsafe {
        let chctrl = ch_reg(ch, OFF_CHCTRL).0;
        crate::mmio::write32(chctrl, CHCTRL_CLREN);
        #[cfg(target_os = "none")]
        core::arch::asm!("dsb", options(nostack));
        crate::mmio::write32(chctrl, CHCTRL_SWRST);
        // Release the peripheral request route too: SWRST resets CHCTRL but not
        // DMARS, so a latched route (e.g. SCIF1-RX = 0x66) would keep claiming
        // its request line and starve the next program's channel. Only this
        // channel's half: the other half routes its pair.
        let dmars = dmars_reg(ch).0;
        let keep = if ch % 2 == 0 {
            0xFFFF_0000
        } else {
            0x0000_FFFF
        };
        crate::mmio::write32(dmars, crate::mmio::read32(dmars) & keep);
    }
}

/// Whether a one-shot transfer started with [`start_transfer`] is still running on channel `ch`:
/// enabled and not yet at its terminal count. Polls the status register, so it works with
/// interrupts masked, where [`wait_transfer_complete`] cannot.
///
/// # Safety
/// Reads a memory-mapped DMA register.
#[inline]
pub(super) unsafe fn transfer_in_flight(ch: u8) -> bool {
    let chstat = unsafe { ch_reg(ch, OFF_CHSTAT).read() };
    chstat & CHSTAT_EN != 0 && chstat & CHSTAT_TC == 0
}

/// Read the current DMA **source** address for channel `ch` (`CRSA_n`).
///
/// For a memory→peripheral transfer (TX), this is the SRAM read pointer.
///
/// # Safety
/// Reads a memory-mapped DMA register.
#[inline]
pub(super) unsafe fn current_src(ch: u8) -> u32 {
    unsafe { ch_reg(ch, OFF_CRSA).read() }
}

/// Read the current DMA **destination** address for channel `ch` (`CRDA_n`).
///
/// For a peripheral→memory transfer (RX), this is the SRAM write pointer.
///
/// # Safety
/// Reads a memory-mapped DMA register.
#[inline]
pub(super) unsafe fn current_dst(ch: u8) -> u32 {
    unsafe { ch_reg(ch, OFF_CRDA).read() }
}

/// Initialise a DMAC channel in **register mode** for a memory→peripheral
/// block transfer.
///
/// Mirrors `oledDMAInit()` from the C firmware:
/// 1. Clears `DCTRL` for the channel's group.
/// 2. Programs `CHCFG`, `CHITVL`, `CHEXT`.
/// 3. Writes the fixed destination address (`N0DA`) — e.g. peripheral data register.
/// 4. Software-resets the channel (clears `TC`).
/// 5. Programs the DMARS resource selector.
///
/// The channel is left idle.  Call [`start_transfer`] for each block; `Dma::new` binds the
/// completion interrupt.
///
/// # Safety
/// Writes to DMAC registers.  Must be called before the channel is used.
pub(super) unsafe fn init_register_mode(ch: u8, chcfg: u32, dst: u32, dmars: u32) {
    unsafe {
        log::debug!(
            "dmac: ch{} register-mode init, chcfg={:#010x}, dst={:#010x}, dmars={:#06x}",
            ch,
            chcfg,
            dst,
            dmars
        );

        // 1. Clear group DCTRL.
        dctrl_reg(ch).write(0);

        // 2. CHCFG / CHITVL / CHEXT.
        ch_reg(ch, OFF_CHCFG).write(chcfg);
        ch_reg(ch, OFF_CHITVL).write(0);
        ch_reg(ch, OFF_CHEXT).write(0);

        // 3. Fixed destination (peripheral register).
        ch_reg(ch, OFF_N0DA).write(dst);

        // 4. Software reset + clear TC.
        // CHCTRL action bits are write-1-to-trigger; write as a clean value
        // (not RMW) to avoid re-triggering actions from stale status read-back.
        let chctrl = ch_reg(ch, OFF_CHCTRL);
        chctrl.write(CHCTRL_SWRST | CHCTRL_CLRTC);
        // Re-write N0DA after reset (matches C firmware's double-write pattern).
        ch_reg(ch, OFF_N0DA).write(dst);

        // 5. DMARS.
        let dmars_ptr = dmars_reg(ch);
        let (shifted, mask) = if ch & 1 == 0 {
            (dmars & 0xFFFF, 0xFFFF_0000u32)
        } else {
            ((dmars & 0xFFFF) << 16, 0x0000_FFFFu32)
        };
        dmars_ptr.write((dmars_ptr.read() & mask) | shifted);
    }
}

/// Call from the DMAINT completion ISR for channel `ch`.
///
/// Wakes any Embassy task waiting in [`wait_transfer_complete`].
pub(super) fn on_dma_int(ch: u8) {
    DMAC_WAKERS[ch as usize].wake();
}

/// Initialise a DMAC channel in **register mode** for a peripheral→memory
/// block transfer.
///
/// Like [`init_register_mode`] but holds the *source* address fixed (e.g.
/// `SD_BUF0`) and lets the destination address increment with each transfer.
///
/// The channel is left idle.  Call [`start_transfer_rx`] for each block.
///
/// # Safety
/// Writes to DMAC registers.  Must be called before the channel is used.
pub(super) unsafe fn init_register_mode_rx(ch: u8, chcfg: u32, src: u32, dmars: u32) {
    unsafe {
        log::debug!(
            "dmac: ch{} register-mode-rx init, chcfg={:#010x}, src={:#010x}, dmars={:#06x}",
            ch,
            chcfg,
            src,
            dmars
        );

        // 1. Clear group DCTRL.
        dctrl_reg(ch).write(0);

        // 2. CHCFG / CHITVL / CHEXT.
        ch_reg(ch, OFF_CHCFG).write(chcfg);
        ch_reg(ch, OFF_CHITVL).write(0);
        ch_reg(ch, OFF_CHEXT).write(0);

        // 3. Fixed source (peripheral register).
        ch_reg(ch, OFF_N0SA).write(src);

        // 4. Software reset + clear TC.
        let chctrl = ch_reg(ch, OFF_CHCTRL);
        chctrl.write(CHCTRL_SWRST | CHCTRL_CLRTC);
        // Re-write N0SA after reset (matches C firmware's double-write pattern).
        ch_reg(ch, OFF_N0SA).write(src);

        // 5. DMARS.
        let dmars_ptr = dmars_reg(ch);
        let (shifted, mask) = if ch & 1 == 0 {
            (dmars & 0xFFFF, 0xFFFF_0000u32)
        } else {
            ((dmars & 0xFFFF) << 16, 0x0000_FFFFu32)
        };
        dmars_ptr.write((dmars_ptr.read() & mask) | shifted);
    }
}

/// Arm and start a one-shot peripheral→memory DMA transfer on a channel
/// initialised with [`init_register_mode_rx`].
///
/// Sets `N0DA = dst`, `N0TB = count`, then writes `CLRTC | SETEN`.
///
/// # Safety
/// The destination region `[dst, dst+count)` must be uncached memory (or have
/// had its cache lines invalidated) so the CPU reads the DMAC-written data.
pub(super) unsafe fn start_transfer_rx(ch: u8, dst: u32, count: u32) {
    unsafe {
        log::trace!(
            "dmac: ch{} start_transfer_rx dst={:#010x} count={}",
            ch,
            dst,
            count
        );
        ch_reg(ch, OFF_N0DA).write(dst);
        ch_reg(ch, OFF_N0TB).write(count);
        let chctrl = ch_reg(ch, OFF_CHCTRL);
        chctrl.write(CHCTRL_CLRTC | CHCTRL_SETEN);
    }
}

/// Arm and start a one-shot DMA transfer on a channel set up with
/// [`init_register_mode`].
///
/// Sets `N0SA = src`, `N0TB = count`, then writes `CLRTC | SETEN` to kick
/// the channel.  Matches the per-frame part of `oledSelectingComplete()`.
///
/// # Safety
/// The source data at `[src, src+count)` must be in uncached memory (or have
/// had its cache lines flushed) so the DMAC reads the correct bytes.
pub(super) unsafe fn start_transfer(ch: u8, src: u32, count: u32) {
    unsafe {
        log::trace!(
            "dmac: ch{} start_transfer src={:#010x} count={}",
            ch,
            src,
            count
        );
        ch_reg(ch, OFF_N0SA).write(src);
        ch_reg(ch, OFF_N0TB).write(count);
        // CHCTRL action bits are write-1-to-trigger; write as a clean value.
        let chctrl = ch_reg(ch, OFF_CHCTRL);
        chctrl.write(CHCTRL_CLRTC | CHCTRL_SETEN);
    }
}

/// Suspend until the DMA transfer on channel `ch` completes.
///
/// Completion is signalled by the DMAINT GIC interrupt (bound by `Dma::new`).  This function must be called **after**
/// [`start_transfer`] has kicked the channel.
pub(super) async fn wait_transfer_complete(ch: u8) {
    poll_fn(|cx| {
        DMAC_WAKERS[ch as usize].register(cx.waker());
        // Check TC flag (CHSTAT.TC = bit 6) directly to avoid missing a
        // completion that arrived before we registered the waker.
        let chstat = unsafe { ch_reg(ch, OFF_CHSTAT).read() };
        if chstat & CHSTAT_TC != 0 {
            // TC bit set — transfer complete
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

/// Await the next per-block END interrupt on a ring channel (bound by `BlockDma::new`).
/// Recurring: call it in a loop.
pub(super) async fn wait_block(ch: u8) {
    poll_fn(|cx| {
        DMAC_WAKERS[ch as usize].register(cx.waker());
        if DMAC_BLOCK_READY[ch as usize].swap(false, Ordering::AcqRel) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

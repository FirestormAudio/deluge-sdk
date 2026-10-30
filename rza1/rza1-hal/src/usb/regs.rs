//! RUSB1 peripheral register map for Renesas RZ/A1L.
//!
//! The registers themselves come from `crate::pac::USB200` / `USB201`, through [`usb`]
//! (and `pipectr` for the DCPCTR / PIPEnCTR family); every access goes through the
//! PAC's `Reg` and so through the MMIO seam.
//!
//! ## Usage
//! ```ignore
//! usb(0).syscfg0().modify(|r| *r |= SYSCFG_USBE);
//! ```
//!
//! ## Bit constants
//! All bitmask constants are declared in this module as `pub(crate) const u16`.
//! Naming follows the C `usb_iobitmask.h` file with the `USB_` prefix removed
//! and snake_case used for the shift variants.

// The bit map is transcribed whole from the manual and `usb_iobitmask.h`, so the next driver
// change finds the bit it needs; not every bit is used today.
#![expect(
    dead_code,
    reason = "the RUSB1 bit map is complete; the drivers use a subset"
)]

use crate::pac::common::{RW, Reg};
use crate::pac::usb::Usb;
use crate::pac::{USB200, USB201};

// ---------------------------------------------------------------------------
// SYSCFG0
// ---------------------------------------------------------------------------
pub(crate) const SYSCFG_USBE: u16 = 0x0001;
pub(crate) const SYSCFG_UPLLE: u16 = 0x0002;
pub(crate) const SYSCFG_UCKSEL: u16 = 0x0004;
pub(crate) const SYSCFG_DPRPU: u16 = 0x0010;
pub(crate) const SYSCFG_DRPD: u16 = 0x0020;
pub(crate) const SYSCFG_DCFM: u16 = 0x0040;
pub(crate) const SYSCFG_HSE: u16 = 0x0080;

pub(crate) const SYSCFG_UCKSEL_SHIFT: u16 = 2;
pub(crate) const SYSCFG_DCFM_SHIFT: u16 = 6;

// ---------------------------------------------------------------------------
// SYSSTS0
// ---------------------------------------------------------------------------
pub(crate) const SYSSTS0_LNST: u16 = 0x0003;

// ---------------------------------------------------------------------------
// DVSTCTR0
// ---------------------------------------------------------------------------
// NOTE: unlike other Renesas USBHS variants (RX, RZ/A1H USBHS), the RZ/A1L
// DVSTCTR0 has NO VBUSEN/EXICEN/HNPBTOA bits — bits 11:9 are reserved
// ("always read as 0, write 0", TRM §28.3.4).  VBUS power must be switched
// externally (board GPIO); the module only *senses* VBUS via the VBUS pin.
pub(crate) const DVSTCTR0_RHST: u16 = 0x0007;
pub(crate) const DVSTCTR0_UACT: u16 = 0x0010;
pub(crate) const DVSTCTR0_RESUME: u16 = 0x0020;
pub(crate) const DVSTCTR0_USBRST: u16 = 0x0040;
pub(crate) const DVSTCTR0_RWUPE: u16 = 0x0080;
pub(crate) const DVSTCTR0_WKUP: u16 = 0x0100;

pub(crate) const DVSTCTR0_RHST_LS: u16 = 0b001;
pub(crate) const DVSTCTR0_RHST_FS: u16 = 0b010;
pub(crate) const DVSTCTR0_RHST_HS: u16 = 0b011;

// ---------------------------------------------------------------------------
// FIFO SEL / CTR  (shared shape between CFIFOSEL and DnFIFOSEL)
// ---------------------------------------------------------------------------
pub(crate) const FIFOSEL_CURPIPE_MASK: u16 = 0x000F;
pub(crate) const FIFOSEL_ISEL: u16 = 0x0020; // CFIFOSEL only
pub(crate) const FIFOSEL_DREQE: u16 = 0x1000; // DnFIFOSEL only: DMA request enable
pub(crate) const FIFOSEL_DCLRM: u16 = 0x2000; // DnFIFOSEL only: clear the buffer once its data is read
pub(crate) const FIFOSEL_BIGEND: u16 = 0x0100;
pub(crate) const FIFOSEL_MBW_MASK: u16 = 0x0C00;
pub(crate) const FIFOSEL_MBW_SHIFT: u32 = 10;
pub(crate) const FIFOSEL_REW: u16 = 0x4000;
pub(crate) const FIFOSEL_RCNT: u16 = 0x8000;

pub(crate) const FIFOCTR_DTLN_MASK: u16 = 0x0FFF;
pub(crate) const FIFOCTR_FRDY: u16 = 0x2000;
pub(crate) const FIFOCTR_BCLR: u16 = 0x4000;
pub(crate) const FIFOCTR_BVAL: u16 = 0x8000;

pub(crate) const MBW_8: u16 = 0;
pub(crate) const MBW_16: u16 = 1;
pub(crate) const MBW_32: u16 = 2;

// ---------------------------------------------------------------------------
// INTENB0
// ---------------------------------------------------------------------------
pub(crate) const INTENB0_BRDYE: u16 = 0x0100;
pub(crate) const INTENB0_NRDYE: u16 = 0x0200;
pub(crate) const INTENB0_BEMPE: u16 = 0x0400;
pub(crate) const INTENB0_CTRE: u16 = 0x0800;
pub(crate) const INTENB0_DVSE: u16 = 0x1000;
pub(crate) const INTENB0_SOFE: u16 = 0x2000;
pub(crate) const INTENB0_RSME: u16 = 0x4000;
pub(crate) const INTENB0_VBSE: u16 = 0x8000;

// ---------------------------------------------------------------------------
// INTENB1 (host-mode attach/detach/setup)
// ---------------------------------------------------------------------------
pub(crate) const INTENB1_SACKE: u16 = 0x0010; // Setup ACK enable
pub(crate) const INTENB1_SIGNE: u16 = 0x0020; // Setup IGNORE (NAK/error) enable
pub(crate) const INTENB1_EOFERRE: u16 = 0x0040;
pub(crate) const INTENB1_ATTCHE: u16 = 0x0800;
pub(crate) const INTENB1_DTCHE: u16 = 0x1000;
pub(crate) const INTENB1_BCHGE: u16 = 0x4000;

// ---------------------------------------------------------------------------
// INTSTS0
// ---------------------------------------------------------------------------
pub(crate) const INTSTS0_CTSQ_MASK: u16 = 0x0007;
pub(crate) const INTSTS0_VALID: u16 = 0x0008;
pub(crate) const INTSTS0_DVSQ_MASK: u16 = 0x0070;
pub(crate) const INTSTS0_DVSQ_SHIFT: u32 = 4;
pub(crate) const INTSTS0_VBSTS: u16 = 0x0080;
pub(crate) const INTSTS0_BRDY: u16 = 0x0100;
pub(crate) const INTSTS0_NRDY: u16 = 0x0200;
pub(crate) const INTSTS0_BEMP: u16 = 0x0400;
pub(crate) const INTSTS0_CTRT: u16 = 0x0800;
pub(crate) const INTSTS0_DVST: u16 = 0x1000;
pub(crate) const INTSTS0_SOFR: u16 = 0x2000;
pub(crate) const INTSTS0_RESM: u16 = 0x4000;
pub(crate) const INTSTS0_VBINT: u16 = 0x8000;

// DVSQ sub-values
pub(crate) const DVSQ_POWERED: u16 = 0b000;
pub(crate) const DVSQ_DEFAULT: u16 = 0b001; // bus reset completed
pub(crate) const DVSQ_ADDRESS: u16 = 0b010;
pub(crate) const DVSQ_CONFIGURED: u16 = 0b011;
pub(crate) const DVSQ_SUSP0: u16 = 0b100;

// CTSQ sub-values
pub(crate) const CTSQ_IDLE: u16 = 0b000;
pub(crate) const CTSQ_READ_DATA: u16 = 0b001;
pub(crate) const CTSQ_READ_STATUS: u16 = 0b010;
pub(crate) const CTSQ_WRITE_DATA: u16 = 0b011;
pub(crate) const CTSQ_WRITE_STATUS: u16 = 0b100;
pub(crate) const CTSQ_WRITE_ZLP: u16 = 0b101;

// ---------------------------------------------------------------------------
// INTSTS1
// ---------------------------------------------------------------------------
pub(crate) const INTSTS1_SACK: u16 = 0x0010; // Setup ACK received
pub(crate) const INTSTS1_SIGN: u16 = 0x0020; // Setup IGNORE (NAK/error)
pub(crate) const INTSTS1_EOFERR: u16 = 0x0040; // EOF error: the controller stopped the port
pub(crate) const INTSTS1_ATTCH: u16 = 0x0800;
pub(crate) const INTSTS1_DTCH: u16 = 0x1000;

// ---------------------------------------------------------------------------
// FRMNUM
// ---------------------------------------------------------------------------
pub(crate) const FRMNUM_FRNM_MASK: u16 = 0x07FF;
pub(crate) const FRMNUM_CRCE: u16 = 0x4000;
pub(crate) const FRMNUM_OVRN: u16 = 0x8000;

// ---------------------------------------------------------------------------
// DCPMAXP
// ---------------------------------------------------------------------------
pub(crate) const DCPMAXP_MXPS_MASK: u16 = 0x007F;
pub(crate) const DCPMAXP_DEVSEL_SHIFT: u32 = 12; // host: device address in DCPMAXP[15:12]

// ---------------------------------------------------------------------------
// DCPCFG (host mode)
// ---------------------------------------------------------------------------
pub(crate) const DCPCFG_DIR: u16 = 0x0010; // bit 4: data stage direction (1=host OUT)
pub(crate) const DCPCFG_SHTNAK: u16 = 0x0080; // NAK on short packet

// ---------------------------------------------------------------------------
// DCPCTR (DCP control — some bits overlap with PIPEnCTR)
// ---------------------------------------------------------------------------
pub(crate) const DCPCTR_SUREQ: u16 = 0x4000; // bit 14: issue SETUP token
pub(crate) const DCPCTR_SUREQCLR: u16 = 0x0800; // bit 11: cancel pending SETUP

// ---------------------------------------------------------------------------
// DCPCTR / PIPEnCTR
// ---------------------------------------------------------------------------
pub(crate) const PIPECTR_PID_MASK: u16 = 0x0003;
pub(crate) const PIPECTR_PID_NAK: u16 = 0b00;
pub(crate) const PIPECTR_PID_BUF: u16 = 0b01;
// STALL has two encodings.  The controller reaches STALL(11) autonomously (on
// babble or a control-sequence error) and firmware sets STALL from NAK→STALL(10)
// or BUF→STALL(11); a direct STALL(11)→NAK write is ignored, so clearing must
// step 11→10→NAK (TRM §28.3.30 PID; matches Linux `__usbhsp_pid_try_nak_if_stall`).
pub(crate) const PIPECTR_PID_STALL10: u16 = 0b10;
pub(crate) const PIPECTR_PID_STALL11: u16 = 0b11;
/// Alias for [`PIPECTR_PID_STALL10`] (the firmware-set STALL encoding).
pub(crate) const PIPECTR_PID_STALL: u16 = PIPECTR_PID_STALL10;
pub(crate) const PIPECTR_CCPL: u16 = 0x0004; // DCP only
pub(crate) const PIPECTR_PBUSY: u16 = 0x0020;
pub(crate) const PIPECTR_SQMON: u16 = 0x0040;
pub(crate) const PIPECTR_SQSET: u16 = 0x0080;
pub(crate) const PIPECTR_SQCLR: u16 = 0x0100;
pub(crate) const PIPECTR_ACLRM: u16 = 0x0200; // pipes 1-9,A-F
pub(crate) const PIPECTR_INBUFM: u16 = 0x4000; // pipes 1-5 and 9-15 (not 6-8)
pub(crate) const PIPECTR_BSTS: u16 = 0x8000;

// ---------------------------------------------------------------------------
// PIPECFG
// ---------------------------------------------------------------------------
pub(crate) const PIPECFG_EPNUM_MASK: u16 = 0x000F;
pub(crate) const PIPECFG_DIR: u16 = 0x0010; // 0=OUT, 1=IN
pub(crate) const PIPECFG_SHTNAK: u16 = 0x0080;
pub(crate) const PIPECFG_CNTMD: u16 = 0x0100;
pub(crate) const PIPECFG_DBLB: u16 = 0x0200;
pub(crate) const PIPECFG_BFRE: u16 = 0x0400;
pub(crate) const PIPECFG_TYPE_MASK: u16 = 0xC000;
pub(crate) const PIPECFG_TYPE_SHIFT: u32 = 14;
pub(crate) const PIPECFG_TYPE_BULK: u16 = 0b01 << 14;
pub(crate) const PIPECFG_TYPE_INTR: u16 = 0b10 << 14;
pub(crate) const PIPECFG_TYPE_ISO: u16 = 0b11 << 14;

// ---------------------------------------------------------------------------
// PIPEBUF
// ---------------------------------------------------------------------------
pub(crate) const PIPEBUF_BUFNMB_MASK: u16 = 0x00FF;
pub(crate) const PIPEBUF_BUFSIZE_MASK: u16 = 0x7C00;
pub(crate) const PIPEBUF_BUFSIZE_SHIFT: u32 = 10;

// ---------------------------------------------------------------------------
// PIPEMAXP
// ---------------------------------------------------------------------------
pub(crate) const PIPEMAXP_MXPS_MASK: u16 = 0x07FF;
pub(crate) const PIPEMAXP_DEVSEL_SHIFT: u32 = 12;

// ---------------------------------------------------------------------------
// PIPEPERI
// ---------------------------------------------------------------------------
// Layout (TRM §28.3.35):
//   [15:13] — reserved
//   [12]    IFIS — isochronous IN buffer flush (host mode: must be 0)
//   [11:3]  — reserved
//   [2:0]   IITV[2:0] — interval as 2^n frames (0..=7)
//
// IITV is only meaningful for interrupt/isochronous pipes (6-9 in host mode).
// For bulk pipes (3-5) and pipes 10-15, the TRM requires IITV=0b000.
//
// Conversion from USB bInterval to IITV (see host::interrupt_iitv):
//   high-speed device:  IITV = (bInterval - 1).min(7)         (microframes)
//   full/low speed:     IITV = floor(log2(bInterval)).min(7)  (1 ms frames, also behind a HS hub)
pub(crate) const PIPEPERI_IFIS: u16 = 1 << 12; // isochronous IN buffer flush enable
pub(crate) const PIPEPERI_IITV_MASK: u16 = 0x0007;

// ---------------------------------------------------------------------------
// DEVADD (host mode — per-device speed + hub configuration)
// ---------------------------------------------------------------------------
// Layout (TRM §28.3 DEVADDn):
//   [15]    — reserved
//   [14:11] UPPHUB[3:0] — USB address of the hub the device is behind
//                          (0000 = directly attached to this port)
//   [10:8]  HUBPORT[2:0] — hub port number
//                          (000 = directly attached to this port)
//   [7:6]   USBSPD[1:0]  — 00=unused, 01=LS, 10=FS, 11=HS
//   [5:0]   — reserved
//
// When UPPHUB/HUBPORT are non-zero the controller automatically issues
// split transactions (SSPLIT/CSPLIT) for the device — no firmware action
// beyond writing the register is needed.
/// USBSPD field in DEVADDn: bits `[7:6]` = 01 → LS, 10 → FS, 11 → HS.
pub(crate) const DEVADD_USBSPD_SHIFT: u32 = 6;
pub(crate) const DEVADD_USBSPD_LS: u16 = 1 << 6; // Low-speed  (01)
pub(crate) const DEVADD_USBSPD_FS: u16 = 2 << 6; // Full-speed (10)
pub(crate) const DEVADD_USBSPD_HS: u16 = 3 << 6; // High-speed (11)
/// UPPHUB field in DEVADDn: bits `[14:11]` — hub USB address (0 = direct).
pub(crate) const DEVADD_UPPHUB_SHIFT: u32 = 11;
/// HUBPORT field in DEVADDn: bits `[10:8]` — hub port number (0 = direct).
pub(crate) const DEVADD_HUBPORT_SHIFT: u32 = 8;

// ---------------------------------------------------------------------------
// SUSPMODE
// ---------------------------------------------------------------------------
pub(crate) const SUSPMODE_SUSPM: u16 = 0x4000;

// ---------------------------------------------------------------------------
// Packet buffer constants  (§28 of the RZ/A1L TRM)
// ---------------------------------------------------------------------------
/// Total packet buffer size in bytes.
pub(crate) const PKT_BUF_SIZE_BYTES: usize = 8192;
/// Granularity of one buffer block in bytes.
pub(crate) const PKT_BUF_BLOCK_SIZE: usize = 64;
/// Total number of 64-byte blocks in the packet buffer.
pub(crate) const PKT_BUF_BLOCKS: usize = PKT_BUF_SIZE_BYTES / PKT_BUF_BLOCK_SIZE; // 128

// ---------------------------------------------------------------------------
// BUSWAIT — BWAIT[5:0] wait cycles per register access (TRM §28.3.2).
// Constraint: consecutive register accesses must take >= 67 ns of P1-bus
// (66.7 MHz, 15 ns/cycle) time, i.e. >= 3 wait cycles (5 access cycles).
// 0x0F (15 waits, the reset value) is used: conservative.
// ---------------------------------------------------------------------------
pub(crate) const BUSWAIT_VALUE: u16 = 0x000F;

// ---------------------------------------------------------------------------
// Registers
// ---------------------------------------------------------------------------

/// USB port `port`'s registers: `USB200` for port 0, `USB201` for port 1.
#[inline]
pub(crate) fn usb(port: u8) -> Usb {
    if port == 0 { USB200 } else { USB201 }
}

/// Pipe `pipe`'s control register: `DCPCTR` for pipe 0, `PIPEnCTR` for pipes 1-15.
#[inline]
pub(crate) fn pipectr(u: Usb, pipe: usize) -> Reg<u16, RW> {
    if pipe == 0 {
        u.dcpctr()
    } else {
        u.pipectr(pipe - 1)
    }
}

// ---------------------------------------------------------------------------
// Address tests
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// The USB registers sit at the offsets `struct st_usb20` (`usb20_iodefine.h`) gives them.
    #[test]
    fn the_pac_names_the_usb_registers() {
        use crate::pac::USB200 as U;
        let pairs = [
            (U.syscfg0().addr(), 0x00),
            (U.buswait().addr(), 0x02),
            (U.syssts0().addr(), 0x04),
            (U.dvstctr0().addr(), 0x08),
            (U.cfifo().addr(), 0x14),
            (U.d0fifo().addr(), 0x18),
            (U.d1fifo().addr(), 0x1C),
            (U.cfifosel().addr(), 0x20),
            (U.cfifoctr().addr(), 0x22),
            (U.d0fifosel().addr(), 0x28),
            (U.d0fifoctr().addr(), 0x2A),
            (U.d1fifosel().addr(), 0x2C),
            (U.d1fifoctr().addr(), 0x2E),
            (U.intenb(0).addr(), 0x30),
            (U.intenb(1).addr(), 0x32),
            (U.brdyenb().addr(), 0x36),
            (U.nrdyenb().addr(), 0x38),
            (U.bempenb().addr(), 0x3A),
            (U.intsts(0).addr(), 0x40),
            (U.intsts(1).addr(), 0x42),
            (U.brdysts().addr(), 0x46),
            (U.nrdysts().addr(), 0x48),
            (U.bempsts().addr(), 0x4A),
            (U.usbaddr().addr(), 0x50),
            (U.usbreq().addr(), 0x54),
            (U.usbval().addr(), 0x56),
            (U.usbindx().addr(), 0x58),
            (U.usbleng().addr(), 0x5A),
            (U.dcpcfg().addr(), 0x5C),
            (U.dcpmaxp().addr(), 0x5E),
            (U.dcpctr().addr(), 0x60),
            (U.pipesel().addr(), 0x64),
            (U.pipecfg().addr(), 0x68),
            (U.pipebuf().addr(), 0x6A),
            (U.pipemaxp().addr(), 0x6C),
            (U.pipeperi().addr(), 0x6E),
            (U.pipectr(0).addr(), 0x70),
            (U.pipectr(14).addr(), 0x8C),
            (U.devadd(0).addr(), 0xD0),
            (U.suspmode().addr(), 0x102),
        ];
        for (i, (addr, off)) in pairs.iter().enumerate() {
            assert_eq!(*addr, 0xE801_0000 + off, "pair {i}");
        }
        assert_eq!(crate::pac::USB201.syscfg0().addr(), 0xE820_7000);
    }

    #[test]
    fn pipectr_maps_pipe_0_to_dcpctr() {
        assert_eq!(pipectr(usb(0), 0).addr(), 0xE801_0060);
        assert_eq!(pipectr(usb(0), 1).addr(), 0xE801_0070);
        assert_eq!(pipectr(usb(1), 15).addr(), 0xE820_708C);
    }
}

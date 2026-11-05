//! The SD card layer over [`Sdhi`]: the SD v2 bring-up protocol and block reads and writes.
//!
//! [`identify`] runs the initialisation sequence once against a controller that is up and in its
//! identification state ([`Sdhi::reset`]):
//!
//! | Step | Command | Purpose |
//! |---|---|---|
//! | 1 | CMD0 | reset the card to idle |
//! | 2 | CMD8 | check the host voltage; answered only by SD v2 cards (SDHC/SDXC capable) |
//! | 3 | ACMD41 (loop) | wait for power-up to finish; learn whether the card is high-capacity |
//! | 4 | CMD2, CMD3 | read the CID (discarded), get the relative card address |
//! | 5 | CMD9 | read the CSD for the card's size ([`Sdhi::set_card_blocks`]) |
//! | 6 | CMD7, ACMD6 | select the card, switch to the 4-bit bus |
//! | 7 | CMD16 | 512-byte blocks (standard-capacity cards) |
//! | 8 | CMD6 | query, then switch to High-Speed; on success SD_CLK goes to P1/2 (33.3 MHz), else stays at P1/4 (16.7 MHz) |
//!
//! It makes one attempt: a card still waking up after a cold power-on needs retries over about a
//! second, which is the caller's policy. [`read_blocks`] and [`write_blocks`] move data through a
//! DMA-visible bounce buffer the caller provides, as many blocks per command as it holds.
//!
//! The controller runs one command at a time: the caller serialises card operations (one lock
//! around each), as [`Sdhi::take_aborted`] explains.

use super::{CLK_DIV_2, Sdhi, SdhiError};
use crate::dma_buf::UncachedMut;

// SD_CMD register values. Most commands are the bare index; the SDHI infers the response type.
// CMD8 sets bits [10:8] (0x0408) to select the R7 response path.
const CMD0: u16 = 0; // GO_IDLE_STATE — no response
const CMD2: u16 = 2; // ALL_SEND_CID — R2
const CMD3: u16 = 3; // SEND_RELATIVE_ADDR — R6
const CMD7: u16 = 7; // SELECT_CARD — R1b
const CMD8: u16 = 0x0408; // SEND_IF_COND — R7 (special encoding)
const CMD9: u16 = 9; // SEND_CSD — R2
const CMD16: u16 = 16; // SET_BLOCKLEN — R1
const CMD17: u16 = 17; // READ_SINGLE_BLOCK — R1 + data
const CMD18: u16 = 18; // READ_MULTIPLE_BLOCK — R1 + data
const CMD24: u16 = 24; // WRITE_BLOCK — R1 + data
const CMD25: u16 = 25; // WRITE_MULTIPLE_BLOCK — R1 + data
const CMD55: u16 = 55; // APP_CMD (prefix for ACMD) — R1

/// CMD6 (SWITCH_FUNC) in extended mode: single-block read, R1 + 64-byte data. SD_CMD encoding
/// 0x1C06 per TRM table 38.7 (MD[2:0]=100 extended/R1, MD3 with data, MD4 read, MD5=0 single
/// block). Normal mode cannot be used: the SDHI does not decode CMD6 as a data command.
const CMD6_DATA: u16 = 0x1C06;
/// CMD6 mode 0 (query): group 1 → function 1, groups 2–6 = 0xF (no change).
const CMD6_ARG_QUERY_HS: u32 = 0x00FF_FF01;
/// CMD6 mode 1 (switch): group 1 to function 1 (High-Speed).
const CMD6_ARG_SWITCH_HS: u32 = 0x80FF_FF01;

/// ACMD6 (0x40 | 6): SET_BUS_WIDTH — R1.
const ACMD6: u16 = 0x40 | 6;
/// ACMD41 (0x40 | 41): SD_SEND_OP_COND — R3.
const ACMD41: u16 = 0x40 | 41;

// A multiple-block READ uses the SDHI's extended transfer mode (SD_CMD[15:8]): bit 13 multiple
// block, bit 12 read, bit 11 with data, [10:8]=100 R1, [15:14]=01 no auto CMD12 (the Renesas
// driver's `CMD18 | 0x7c00`). Single-block reads and every write use normal mode, the bare index,
// as the vendor driver does. High- and standard-capacity cards differ only in the argument (block
// or byte address, `Card::block_address`), never the encoding: OR-ing 0x7C00 into another command
// forces multiple-block read mode, so a CMD17 would wait for a second block until the data timeout
// and a write would get the read-direction bit.
const CMD18_MULTI: u16 = CMD18 | 0x7C00;

/// OCR voltage window: 3.2–3.4 V (SD_VOLT_3_3).
const OCR_VDD_32_33: u32 = 0x0010_0000;
/// OCR host capacity support (HCS): high-capacity cards.
const OCR_HCS: u32 = 0x4000_0000;
/// OCR power-up status: set once the card is ready.
const OCR_BUSY: u32 = 0x8000_0000;
/// CMD8 argument: VHS = 1 (2.7–3.6 V) and check pattern 0xAA.
const CMD8_ARG: u32 = 0x0000_01AA;
/// ACMD41 argument: HCS and the voltage window.
const ACMD41_ARG: u32 = OCR_HCS | OCR_VDD_32_33;

/// A card [`identify`] brought to the transfer state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Card {
    rca: u16,
    high_capacity: bool,
    high_speed: bool,
}

impl Card {
    /// The relative card address CMD3 assigned.
    pub fn rca(&self) -> u16 {
        self.rca
    }

    /// Whether the card is high-capacity (SDHC/SDXC), and so block-addressed.
    pub fn is_high_capacity(&self) -> bool {
        self.high_capacity
    }

    /// Whether the card switched to High-Speed (SD_CLK 33.3 MHz).
    pub fn is_high_speed(&self) -> bool {
        self.high_speed
    }

    /// The command argument addressing block `lba`: the block number on a high-capacity card, the
    /// byte offset on a standard-capacity one. The byte offset saturates instead of overflowing, so
    /// a bogus high block number reaches the card as an address it rejects.
    pub fn block_address(&self, lba: u32) -> u32 {
        if self.high_capacity {
            lba
        } else {
            lba.saturating_mul(512)
        }
    }
}

/// Why a card operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum CardError {
    /// The controller reported an error or timed out.
    Sdhi(SdhiError),
    /// The card isn't a usable SD card (it never finished powering up, or lacks a function asked
    /// for).
    Unsupported,
    /// The card answered out of protocol.
    Protocol,
    /// The buffer is smaller than the blocks asked for, or the bounce buffer holds no whole block.
    BufferTooSmall,
}

impl From<SdhiError> for CardError {
    fn from(e: SdhiError) -> Self {
        CardError::Sdhi(e)
    }
}

/// Decoded fields of the CMD6 (SWITCH_FUNC) 512-bit status block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchStatus {
    /// Function group 1 (access mode) supports function 1 (High-Speed).
    pub hs_supported: bool,
    /// Function group 1's selected-function nibble (0x1 = High-Speed selected, 0xF = switch
    /// refused / function error).
    pub group1_selected: u8,
}

/// Parse the CMD6 switch-function status block. It arrives MSB first: `buf[0]` holds status bits
/// 511:504. Group 1's support mask is bits 415:400 (bytes 12–13; function 1 = bit 401 = byte 13
/// bit 1), and its selected function bits 379:376 (byte 16's low nibble).
pub fn parse_switch_status(buf: &[u8; 64]) -> SwitchStatus {
    SwitchStatus {
        hs_supported: buf[13] & 0x02 != 0,
        group1_selected: buf[16] & 0x0F,
    }
}

/// The card's size in 512-byte blocks, from its CSD (`csd[0]` = bits 127:96).
///
/// CSD v2 (high capacity): `C_SIZE[69:48]`; (C_SIZE + 1) × 512 KiB. CSD v1 (standard capacity):
/// (`C_SIZE[73:62]` + 1) × 2^(`C_SIZE_MULT[49:47]` + 2) × 2^`READ_BL_LEN[83:80]` bytes.
pub fn decode_csd_capacity(csd: [u32; 4], high_capacity: bool) -> u32 {
    if high_capacity {
        // C_SIZE spans word1[5:0] (bits 69:64) and word2[31:16] (bits 63:48).
        let c_size = ((csd[1] & 0x3F) << 16) | ((csd[2] >> 16) & 0xFFFF);
        (c_size + 1) * 1024
    } else {
        // READ_BL_LEN word1[19:16]; C_SIZE word1[9:0] ++ word2[31:30]; C_SIZE_MULT word2[17:15].
        let read_bl_len = (csd[1] >> 16) & 0xF;
        let c_size = ((csd[1] & 0x3FF) << 2) | ((csd[2] >> 30) & 0x3);
        let c_size_mult = (csd[2] >> 15) & 0x7;
        let capacity_bytes =
            (c_size as u64 + 1) * (1u64 << (c_size_mult + 2)) * (1u64 << read_bl_len);
        (capacity_bytes / 512) as u32
    }
}

/// CMD55 (APP_CMD) addressed to `rca`, then `acmd`.
async fn send_acmd(sdhi: &Sdhi<'_>, rca: u16, acmd: u16, arg: u32) -> Result<(), CardError> {
    sdhi.set_arg((rca as u32) << 16);
    sdhi.send_cmd(CMD55).await?;
    sdhi.set_arg(arg);
    sdhi.send_cmd(acmd).await?;
    Ok(())
}

/// The CMD6 switch to High-Speed (SD spec 1.10 and later), with the card in the transfer state.
/// Any error leaves the card at default speed; pre-1.10 cards reject CMD6 as illegal.
async fn try_high_speed(sdhi: &Sdhi<'_>) -> Result<(), CardError> {
    let mut status = [0u8; 64];
    sdhi.read_status_block_sw(CMD6_DATA, CMD6_ARG_QUERY_HS, &mut status)
        .await?;
    if !parse_switch_status(&status).hs_supported {
        return Err(CardError::Unsupported);
    }
    sdhi.read_status_block_sw(CMD6_DATA, CMD6_ARG_SWITCH_HS, &mut status)
        .await?;
    if parse_switch_status(&status).group1_selected != 0x1 {
        return Err(CardError::Protocol);
    }
    Ok(())
}

/// Bring the card in the slot to the transfer state, once (see the module docs). Records the
/// card's size with [`Sdhi::set_card_blocks`] and leaves SD_CLK at the data rate it reached.
pub async fn identify(sdhi: &Sdhi<'_>) -> Result<Card, CardError> {
    // CMD0: reset to idle. No response; a timeout is expected.
    sdhi.set_arg(0);
    let _ = sdhi.send_cmd(CMD0).await;
    embassy_time::Timer::after_millis(1).await;

    // CMD8: voltage check; only SD v2 cards answer.
    sdhi.set_arg(CMD8_ARG);
    let cmd8_ok = sdhi.send_cmd(CMD8).await.is_ok();

    // ACMD41 until the card clears OCR busy. During identification the card answers to RCA 0.
    let hcs_arg = if cmd8_ok { ACMD41_ARG } else { OCR_VDD_32_33 };
    let mut retries = 0u32;
    let ocr = loop {
        send_acmd(sdhi, 0, ACMD41, hcs_arg).await?;
        let ocr = sdhi.read_r1(); // the R3 OCR comes through the same registers
        if ocr & OCR_BUSY != 0 {
            break ocr;
        }
        retries += 1;
        if retries > 1000 {
            return Err(CardError::Unsupported);
        }
        embassy_time::Timer::after_millis(1).await;
    };
    let high_capacity = cmd8_ok && (ocr & OCR_HCS != 0);
    debug!("sd: cmd8_ok={} ocr={:x} hc={}", cmd8_ok, ocr, high_capacity);

    // CMD2: the CID, discarded. CMD3: the RCA (R6 [31:16]).
    sdhi.set_arg(0);
    sdhi.send_cmd(CMD2).await?;
    let _ = sdhi.read_r2();
    sdhi.set_arg(0);
    sdhi.send_cmd(CMD3).await?;
    let rca = (sdhi.read_r1() >> 16) as u16;

    // CMD9 (stand-by state, argument RCA << 16): the CSD, for the card's size. The SDHI returns R2
    // as CSD[127:8] right-justified (the CRC byte stripped), so shift the words left 8 to restore
    // the 128-bit layout `decode_csd_capacity` reads.
    sdhi.set_arg((rca as u32) << 16);
    if sdhi.send_cmd(CMD9).await.is_ok() {
        let raw = sdhi.read_r2();
        let csd = [
            (raw[0] << 8) | (raw[1] >> 24),
            (raw[1] << 8) | (raw[2] >> 24),
            (raw[2] << 8) | (raw[3] >> 24),
            raw[3] << 8,
        ];
        let total = decode_csd_capacity(csd, high_capacity);
        debug!(
            "sd: CSD={:x} {:x} {:x} {:x} -> {} blocks",
            csd[0], csd[1], csd[2], csd[3], total
        );
        sdhi.set_card_blocks(total);
    }

    // CMD7: select (transfer state). ACMD6: 4-bit bus.
    sdhi.set_arg((rca as u32) << 16);
    sdhi.send_cmd(CMD7).await?;
    send_acmd(sdhi, rca, ACMD6, 0x2).await?;

    // CMD16: 512-byte blocks on a standard-capacity card.
    if !high_capacity {
        sdhi.set_arg(512);
        sdhi.send_cmd(CMD16).await?;
    }

    // The default-speed data clock before CMD6: a transfer-mode command must not run at the
    // ~130 kHz identification clock (some cards never deliver its data block there, and the
    // data-phase timeout is then minutes long). 16.7 MHz is legal for any card in transfer state.
    sdhi.set_clock_fast();

    // CMD6 to High-Speed (33.3 MHz), else stay at 16.7 MHz. The card allows the new timing 8
    // clocks after the switch status's end bit, which DATA_TRNS is already past.
    let high_speed = match try_high_speed(sdhi).await {
        Ok(()) => {
            sdhi.set_clock_div(CLK_DIV_2);
            info!("sd: High-Speed mode, SD_CLK = 33.3 MHz (P1/2)");
            true
        }
        Err(e) => {
            info!("sd: default speed, SD_CLK = 16.7 MHz (P1/4): {:?}", e);
            false
        }
    };

    Ok(Card {
        rca,
        high_capacity,
        high_speed,
    })
}

/// How many whole blocks `bounce` holds, and the byte count `count` blocks need.
fn plan(count: u32, len: usize, bounce: &UncachedMut<'_>) -> Result<(u32, usize), CardError> {
    let per_command = (bounce.len() / 512) as u32;
    let bytes = (count as usize)
        .checked_mul(512)
        .ok_or(CardError::BufferTooSmall)?;
    if per_command == 0 || len < bytes {
        return Err(CardError::BufferTooSmall);
    }
    Ok((per_command, bytes))
}

/// Read `count` blocks from `lba` into `buf` (at least `count * 512` bytes), through `bounce`, as
/// many blocks per command as it holds: CMD17 for one, CMD18 with a manual CMD12 for more.
pub async fn read_blocks(
    sdhi: &Sdhi<'_>,
    card: &Card,
    lba: u32,
    count: u32,
    buf: &mut [u8],
    bounce: &mut UncachedMut<'_>,
) -> Result<(), CardError> {
    let (per_command, _) = plan(count, buf.len(), bounce)?;
    let mut done = 0u32;
    while done < count {
        let chunk = (count - done).min(per_command);
        let bytes = (chunk as usize) * 512;
        let cmd = if chunk > 1 { CMD18_MULTI } else { CMD17 };
        sdhi.set_block_count(chunk);
        sdhi.set_arg(card.block_address(lba.saturating_add(done)));
        let xfer = sdhi.read_blocks_dma(cmd, bounce.prefix(bytes)).await;
        // CMD18 runs with auto-CMD12 off, so the card streams until STOP_TRANSMISSION; SD_STOP
        // only bounds what the controller clocks in. Stop it even when the data phase failed, or
        // the card keeps streaming and the next command times out; report the transfer's error
        // first.
        let stop = if chunk > 1 {
            sdhi.stop_transfer().await
        } else {
            Ok(())
        };
        xfer?;
        stop?;
        let at = (done as usize) * 512;
        buf[at..at + bytes].copy_from_slice(&bounce.as_slice()[..bytes]);
        done += chunk;
    }
    Ok(())
}

/// Write `count` blocks from `buf` (at least `count * 512` bytes) to `lba`, through `bounce`, as
/// many blocks per command as it holds: CMD24 for one, CMD25 for more.
pub async fn write_blocks(
    sdhi: &Sdhi<'_>,
    card: &Card,
    lba: u32,
    count: u32,
    buf: &[u8],
    bounce: &mut UncachedMut<'_>,
) -> Result<(), CardError> {
    let (per_command, _) = plan(count, buf.len(), bounce)?;
    let mut done = 0u32;
    while done < count {
        let chunk = (count - done).min(per_command);
        let bytes = (chunk as usize) * 512;
        let at = (done as usize) * 512;
        bounce.as_mut_slice()[..bytes].copy_from_slice(&buf[at..at + bytes]);
        let cmd = if chunk > 1 { CMD25 } else { CMD24 };
        sdhi.set_block_count(chunk);
        sdhi.set_arg(card.block_address(lba.saturating_add(done)));
        sdhi.write_blocks_dma(cmd, bounce.prefix(bytes).as_source())
            .await?;
        done += chunk;
    }
    Ok(())
}

/// Read `count` blocks from `lba` straight into `buf` (exactly `count * 512` bytes) by
/// interrupt-driven PIO, with no bounce buffer, as the vendor driver's `_sd_software_trans` does:
/// for a caller that can't provide DMA-visible memory. CMD18 with a manual CMD12 for more than one
/// block.
pub async fn read_blocks_pio(
    sdhi: &Sdhi<'_>,
    card: &Card,
    lba: u32,
    count: u32,
    buf: &mut [u8],
) -> Result<(), CardError> {
    if count == 0 {
        return Ok(());
    }
    if buf.len() != (count as usize) * 512 {
        return Err(CardError::BufferTooSmall);
    }
    let cmd = if count > 1 { CMD18_MULTI } else { CMD17 };
    sdhi.set_block_count(count);
    sdhi.set_arg(card.block_address(lba));
    sdhi.send_cmd(cmd).await?;
    let xfer = sdhi.read_blocks_sw(buf).await;
    // As in `read_blocks`: stop a multiple-block read even when its data phase failed.
    let stop = if count > 1 {
        sdhi.stop_transfer().await
    } else {
        Ok(())
    };
    xfer?;
    stop?;
    Ok(())
}

/// Write `count` blocks from `buf` (exactly `count * 512` bytes) to `lba` by interrupt-driven PIO,
/// with no bounce buffer: CMD24 for one block, CMD25 for more.
pub async fn write_blocks_pio(
    sdhi: &Sdhi<'_>,
    card: &Card,
    lba: u32,
    count: u32,
    buf: &[u8],
) -> Result<(), CardError> {
    if count == 0 {
        return Ok(());
    }
    if buf.len() != (count as usize) * 512 {
        return Err(CardError::BufferTooSmall);
    }
    let cmd = if count > 1 { CMD25 } else { CMD24 };
    sdhi.set_block_count(count);
    sdhi.set_arg(card.block_address(lba));
    sdhi.send_cmd(cmd).await?;
    sdhi.write_blocks_sw(buf).await?;
    Ok(())
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn hs_supported_and_selected() {
        let mut buf = [0u8; 64];
        buf[13] = 0x03; // group 1 supports functions 0 and 1
        buf[16] = 0x01; // group 1 switched to function 1 (High-Speed)
        let st = parse_switch_status(&buf);
        assert!(st.hs_supported);
        assert_eq!(st.group1_selected, 0x1);
    }

    #[test]
    fn hs_unsupported() {
        let mut buf = [0u8; 64];
        buf[13] = 0x01; // only function 0 (default speed)
        let st = parse_switch_status(&buf);
        assert!(!st.hs_supported);
        assert_eq!(st.group1_selected, 0x0);
    }

    #[test]
    fn switch_refused_is_0xf_and_nibbles_do_not_leak() {
        let mut buf = [0u8; 64];
        buf[13] = 0x03;
        buf[16] = 0x0F; // 0xF = function error / switch refused
        assert_eq!(parse_switch_status(&buf).group1_selected, 0xF);
        buf[16] = 0xF1; // upper nibble belongs to group 2 — must not leak
        assert_eq!(parse_switch_status(&buf).group1_selected, 0x1);
    }

    #[test]
    fn csd_v2_capacity_is_c_size_plus_one_times_1024_blocks() {
        // CSD_STRUCTURE = 1; C_SIZE = 0x3B37 (an 8 GB card), all in word2[31:16].
        let csd = [0x4000_0000, 0, 0x3B37_0000, 0];
        assert_eq!(decode_csd_capacity(csd, true), (0x3B37 + 1) * 1024);
        // C_SIZE's top six bits sit in word1[5:0].
        let csd = [0x4000_0000, 0x12, 0x3456_0000, 0];
        assert_eq!(decode_csd_capacity(csd, true), (0x12_3456 + 1) * 1024);
    }

    #[test]
    fn csd_v1_capacity_follows_c_size_mult_and_block_length() {
        // READ_BL_LEN = 9 (512 B), C_SIZE = 3863, C_SIZE_MULT = 7: (3864 × 512 × 512) / 512 blocks.
        let csd = [0, 0x0009_03C5, 0xC003_8000, 0];
        assert_eq!(decode_csd_capacity(csd, false), 1_978_368);
    }

    #[test]
    fn standard_capacity_cards_are_byte_addressed_and_saturate() {
        let sdsc = Card {
            rca: 1,
            high_capacity: false,
            high_speed: false,
        };
        assert_eq!(sdsc.block_address(3), 3 * 512);
        assert_eq!(sdsc.block_address(u32::MAX), u32::MAX);
        let sdhc = Card {
            high_capacity: true,
            ..sdsc
        };
        assert_eq!(sdhc.block_address(3), 3);
    }
}

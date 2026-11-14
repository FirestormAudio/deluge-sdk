//! `USB UPDATE`'s logic: putting a UF2 file back together from the sectors a host writes, in
//! whatever order, and checking what it carries before anything is written to flash.
//!
//! The host writes the `.uf2` file's sectors among its own housekeeping (directory entries, FAT
//! updates, `._*` files). [`Assembler`] keeps the UF2 blocks whose family is this board's, copies
//! each payload to its offset in an SDRAM buffer, and says when every block of the file is in.
//! [`validate`] then checks the image as the slot it goes to would. Nothing here writes flash.

use crate::boot::Form;
use crate::stage::flash_app_move;
use deluge_image::app::{APP_HEADER_LEN, AppError, validate_app};
use deluge_image::slot::{HEADER_LEN, SlotError, SlotLayout, validate_with};
use deluge_image::uf2::{BLOCK_LEN, Families, MAX_BLOCKS, decode};

pub mod volume;

/// Where a file goes, from its family ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The flash app slot.
    App,
    /// The inactive boot slot, through the loader's self-update.
    Loader,
}

/// Why a file is refused. Nothing has been written when one is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Another board's (or display's) file.
    WrongBoard,
    /// A block or the image doesn't check out.
    Damaged,
    /// Bigger than where it goes.
    TooLarge,
    /// A loader file, but this loader wasn't started from a boot slot (the Deluge's stock form).
    NeedsSlotBoot,
}

impl Refusal {
    /// The second line the display shows.
    pub fn message(self) -> &'static [u8] {
        match self {
            Refusal::WrongBoard => b"WRONG BOARD",
            Refusal::Damaged => b"DAMAGED FILE",
            Refusal::TooLarge => b"TOO LARGE",
            Refusal::NeedsSlotBoot => b"NEEDS SLOT BOOT",
        }
    }
}

/// What a written sector did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Not a block of a file this board takes (host housekeeping), or more of a file already
    /// refused.
    Ignored,
    /// `received` of the file's `total` blocks are in.
    Progress { received: u32, total: u32 },
    /// Every block of the file is in: [`Assembler::image`] is its `len` bytes.
    Complete { target: Target, len: usize },
    /// The file is refused; reported once per file, at its first offending block.
    Refused(Refusal),
}

/// What this board takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub families: Families,
    /// The largest app image, its DAPP header included.
    pub max_app: usize,
    /// The largest loader slot image, its header page included.
    pub max_loader: usize,
}

const WORDS: usize = (MAX_BLOCKS as usize).div_ceil(32);

/// A UF2 file put back together in `buf`, block by block.
pub struct Assembler<'b> {
    buf: &'b mut [u8],
    limits: Limits,
    /// The file being received: (family, `numBlocks`).
    file: Option<(u32, u32)>,
    /// The file last refused: (family, `numBlocks`). Its later blocks are ignored, until a
    /// block 0 (the file dropped again) clears it.
    refused: Option<(u32, u32)>,
    bits: [u32; WORDS],
    received: u32,
    len: usize,
    /// The file is complete: it's kept, and every later sector ignored, until [`Assembler::reset`]
    /// (a host write can carry more sectors after the one that completed it).
    complete: bool,
}

impl<'b> Assembler<'b> {
    pub fn new(buf: &'b mut [u8], limits: Limits) -> Self {
        Self {
            buf,
            limits,
            file: None,
            refused: None,
            bits: [0; WORDS],
            received: 0,
            len: 0,
            complete: false,
        }
    }

    /// Forget everything received or refused: a new session.
    pub fn reset(&mut self) {
        self.file = None;
        self.refused = None;
        self.bits = [0; WORDS];
        self.received = 0;
        self.len = 0;
        self.complete = false;
    }

    /// (blocks received, the file's block count); `(0, 0)` before a block of a file arrives.
    pub fn progress(&self) -> (u32, u32) {
        match self.file {
            Some((_, n)) => (self.received, n),
            None => (0, 0),
        }
    }

    /// The bytes received so far: the whole image once [`Event::Complete`] is reported.
    pub fn image(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// As [`Assembler::image`], writable (a self-update rewrites the slot header in place).
    pub fn image_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.len]
    }

    /// Take one sector the host wrote.
    pub fn write(&mut self, sector: &[u8; BLOCK_LEN]) -> Event {
        if self.complete {
            return Event::Ignored;
        }
        let Some(b) = decode(sector) else {
            return Event::Ignored;
        };
        let id = (b.family, b.num_blocks);
        if b.block_no == 0 && self.refused == Some(id) {
            self.refused = None;
        }
        if self.refused == Some(id) {
            return Event::Ignored;
        }
        let refuse = |s: &mut Self, r| {
            s.refused = Some(id);
            Event::Refused(r)
        };
        let (target, max) = if b.family == self.limits.families.app {
            (Target::App, self.limits.max_app)
        } else if b.family == self.limits.families.loader {
            (Target::Loader, self.limits.max_loader)
        } else {
            return refuse(self, Refusal::WrongBoard);
        };
        if b.num_blocks > MAX_BLOCKS {
            return refuse(self, Refusal::TooLarge);
        }
        if b.block_no >= b.num_blocks {
            return refuse(self, Refusal::Damaged);
        }
        // In u64: a `targetAddr` near the top of the 32-bit range mustn't wrap on the device.
        let end = u64::from(b.target) + b.payload.len() as u64;
        if end > max.min(self.buf.len()) as u64 {
            return refuse(self, Refusal::TooLarge);
        }
        let (start, end) = (b.target as usize, end as usize);
        let (word, bit) = (b.block_no as usize / 32, 1u32 << (b.block_no % 32));
        if self.file != Some(id) {
            self.file = Some(id);
            self.bits = [0; WORDS];
            self.received = 0;
            self.len = 0;
        }
        self.buf[start..end].copy_from_slice(b.payload);
        if self.bits[word] & bit == 0 {
            self.bits[word] |= bit;
            self.received += 1;
        }
        self.len = self.len.max(end);
        if self.received == b.num_blocks {
            self.complete = true;
            Event::Complete {
                target,
                len: self.len,
            }
        } else {
            Event::Progress {
                received: self.received,
                total: b.num_blocks,
            }
        }
    }
}

/// What a completed file is checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    /// How this loader was started: a loader file needs a boot slot to go to.
    pub form: Form,
    /// The variant code both an app and a loader must carry.
    pub variant: u16,
    /// The boot slots' layout, for a loader.
    pub layout: SlotLayout,
    /// The largest app body the app slot holds.
    pub max_app_body: u32,
    /// The RAM an app's body must load into, wholly.
    pub app_ram: core::ops::Range<u32>,
}

/// Check a completed file as where it goes would, before anything is written: an app as a DAPP
/// image of the variant, with a body that fits the slot and loads wholly inside `app_ram`; a
/// loader as a boot-slot image of the variant, and only when this loader was started from a boot
/// slot. `Ok` is how many of `image`'s bytes to write (trailing bytes are the file's padding).
pub fn validate(target: Target, image: &[u8], c: &Checks) -> Result<usize, Refusal> {
    match target {
        Target::App => {
            let h = validate_app(image, &[c.variant], c.max_app_body).map_err(|e| match e {
                AppError::BadVariant => Refusal::WrongBoard,
                _ => Refusal::Damaged,
            })?;
            // Where it loads: an app that can't be launched mustn't replace one that can.
            flash_app_move(&h, 0, c.app_ram.clone()).map_err(|_| Refusal::Damaged)?;
            Ok(APP_HEADER_LEN + h.body_len as usize)
        }
        Target::Loader => {
            if c.form == Form::Stock {
                return Err(Refusal::NeedsSlotBoot);
            }
            validate_with(image, c.layout, &[c.variant])
                .map(|h| HEADER_LEN + h.body_len as usize)
                .map_err(|e| match e {
                    SlotError::BadVariant => Refusal::WrongBoard,
                    _ => Refusal::Damaged,
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_image::app::encode_app_header;
    use deluge_image::slot::{
        self, DELUGE, HEADER_LEN, Reason, VARIANT_OLED, VARIANT_SEVEN_SEG, encode_header_with,
    };
    use deluge_image::uf2::{self, BLOCK_LEN, DELUGE_OLED, DELUGE_SEVEN_SEG, MAX_BLOCKS};
    use std::vec;
    use std::vec::Vec;

    const LOAD: u32 = 0x2002_0000;
    const SLOT_FORM: Form = Form::Slot {
        slot: 0,
        reason: Reason::OnlyValid,
    };

    fn app(len: usize, variant: u16) -> Vec<u8> {
        let body: Vec<u8> = (0..len).map(|i| (i * 13 + 5) as u8).collect();
        let mut v = encode_app_header(&body, variant, LOAD, LOAD).to_vec();
        v.extend_from_slice(&body);
        v
    }

    fn loader(len: usize) -> Vec<u8> {
        let body: Vec<u8> = (0..len).map(|i| (i * 3 + 1) as u8).collect();
        let mut v = encode_header_with(&body, VARIANT_OLED, 1, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        v.extend_from_slice(&body);
        v
    }

    fn checks(form: Form) -> Checks {
        Checks {
            form,
            variant: VARIANT_OLED,
            layout: DELUGE,
            max_app_body: 0x2C_0000 - 64,
            app_ram: 0x2002_0000..0x2030_0000,
        }
    }

    fn limits() -> Limits {
        Limits {
            families: DELUGE_OLED,
            max_app: 0x2C_0000,
            max_loader: slot::SLOT_LEN as usize,
        }
    }

    fn blocks(image: &[u8], family: u32) -> Vec<[u8; BLOCK_LEN]> {
        uf2::encode(image, family).collect()
    }

    /// Feed every block; the last event.
    fn feed(a: &mut Assembler, bs: &[[u8; BLOCK_LEN]]) -> Event {
        let mut last = Event::Ignored;
        for b in bs {
            last = a.write(b);
        }
        last
    }

    #[test]
    fn in_order_completes() {
        let image = app(1000, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let bs = blocks(&image, DELUGE_OLED.app);
        assert_eq!(
            a.write(&bs[0]),
            Event::Progress {
                received: 1,
                total: bs.len() as u32
            }
        );
        assert_eq!(
            feed(&mut a, &bs[1..]),
            Event::Complete {
                target: Target::App,
                len: image.len()
            }
        );
        assert_eq!(a.image(), &image[..]);
    }

    #[test]
    fn out_of_order_and_duplicates_complete() {
        let image = app(2000, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let mut bs = blocks(&image, DELUGE_OLED.app);
        bs.reverse();
        let dup = bs[bs.len() - 2];
        bs.insert(1, dup);
        assert_eq!(
            feed(&mut a, &bs),
            Event::Complete {
                target: Target::App,
                len: image.len()
            }
        );
        assert_eq!(a.image(), &image[..]);
    }

    #[test]
    fn junk_sectors_ignored() {
        let image = app(1500, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let bs = blocks(&image, DELUGE_OLED.app);
        let zero = [0u8; BLOCK_LEN];
        let noise: [u8; BLOCK_LEN] = core::array::from_fn(|i| (i * 31 + 7) as u8);
        let mut last = Event::Ignored;
        for b in &bs {
            assert_eq!(a.write(&zero), Event::Ignored);
            assert_eq!(a.write(&noise), Event::Ignored);
            last = a.write(b);
        }
        assert_eq!(
            last,
            Event::Complete {
                target: Target::App,
                len: image.len()
            }
        );
        assert_eq!(a.image(), &image[..]);
    }

    #[test]
    fn loader_family_targets_loader() {
        let image = loader(3000);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        assert_eq!(
            feed(&mut a, &blocks(&image, DELUGE_OLED.loader)),
            Event::Complete {
                target: Target::Loader,
                len: image.len()
            }
        );
    }

    #[test]
    fn foreign_file_reports_wrong_board_once() {
        let image = app(1000, VARIANT_SEVEN_SEG);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let bs = blocks(&image, DELUGE_SEVEN_SEG.app);
        assert_eq!(a.write(&bs[0]), Event::Refused(Refusal::WrongBoard));
        for b in &bs[1..] {
            assert_eq!(a.write(b), Event::Ignored);
        }
    }

    #[test]
    fn new_file_restarts() {
        let a_img = app(3000, VARIANT_OLED);
        let b_img = app(1000, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let a_bs = blocks(&a_img, DELUGE_OLED.app);
        feed(&mut a, &a_bs[..a_bs.len() / 2]);
        assert_eq!(
            feed(&mut a, &blocks(&b_img, DELUGE_OLED.app)),
            Event::Complete {
                target: Target::App,
                len: b_img.len()
            }
        );
        assert_eq!(a.image(), &b_img[..]);
    }

    #[test]
    fn reset_forgets_partial() {
        let image = app(2000, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let bs = blocks(&image, DELUGE_OLED.app);
        feed(&mut a, &bs[..4]);
        a.reset();
        assert_eq!(a.progress(), (0, 0));
        assert_eq!(
            a.write(&bs[0]),
            Event::Progress {
                received: 1,
                total: bs.len() as u32
            }
        );
    }

    /// A block of `family` for a file of `num_blocks`, number `block_no`, at `target`.
    fn crafted(family: u32, block_no: u32, num_blocks: u32, target: u32) -> [u8; BLOCK_LEN] {
        let mut b = blocks(&[0xAA; 16], family)[0];
        b[12..16].copy_from_slice(&target.to_le_bytes());
        b[20..24].copy_from_slice(&block_no.to_le_bytes());
        b[24..28].copy_from_slice(&num_blocks.to_le_bytes());
        b
    }

    #[test]
    fn too_many_blocks_is_too_large() {
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let b = crafted(DELUGE_OLED.app, 0, MAX_BLOCKS + 1, 0);
        assert_eq!(a.write(&b), Event::Refused(Refusal::TooLarge));
    }

    #[test]
    fn target_past_max_is_too_large() {
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let b = crafted(DELUGE_OLED.app, 0, 2, 0x2C_0000 - 8);
        assert_eq!(a.write(&b), Event::Refused(Refusal::TooLarge));
    }

    #[test]
    fn loader_past_slot_is_too_large() {
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let b = crafted(DELUGE_OLED.loader, 0, 2, slot::SLOT_LEN);
        assert_eq!(a.write(&b), Event::Refused(Refusal::TooLarge));
    }

    #[test]
    fn target_past_buffer_is_too_large() {
        let mut buf = vec![0u8; 1024];
        let mut a = Assembler::new(&mut buf, limits());
        let b = crafted(DELUGE_OLED.app, 0, 2, 1020);
        assert_eq!(a.write(&b), Event::Refused(Refusal::TooLarge));
    }

    #[test]
    fn block_no_out_of_range_is_damaged() {
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let b = crafted(DELUGE_OLED.app, 2, 2, 0);
        assert_eq!(a.write(&b), Event::Refused(Refusal::Damaged));
    }

    #[test]
    fn holey_image_fails_validation() {
        let image = app(2000, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let mut bs = blocks(&image, DELUGE_OLED.app);
        bs[3][0x20..0x20 + 256].fill(0);
        assert!(matches!(feed(&mut a, &bs), Event::Complete { .. }));
        assert_eq!(
            validate(Target::App, a.image(), &checks(SLOT_FORM)),
            Err(Refusal::Damaged)
        );
    }

    #[test]
    fn wrong_variant_app_is_wrong_board() {
        let image = app(500, VARIANT_SEVEN_SEG);
        assert_eq!(
            validate(Target::App, &image, &checks(SLOT_FORM)),
            Err(Refusal::WrongBoard)
        );
    }

    #[test]
    fn loader_in_stock_form_needs_slot_boot() {
        let image = loader(500);
        assert_eq!(
            validate(Target::Loader, &image, &checks(Form::Stock)),
            Err(Refusal::NeedsSlotBoot)
        );
    }

    #[test]
    fn damaged_loader_is_damaged() {
        let mut image = loader(500);
        image[HEADER_LEN + 10] ^= 1;
        assert_eq!(
            validate(Target::Loader, &image, &checks(SLOT_FORM)),
            Err(Refusal::Damaged)
        );
    }

    #[test]
    fn good_app_and_loader_validate() {
        assert_eq!(
            validate(Target::App, &app(700, VARIANT_OLED), &checks(SLOT_FORM)),
            Ok(64 + 700)
        );
        assert_eq!(
            validate(Target::Loader, &loader(700), &checks(SLOT_FORM)),
            Ok(HEADER_LEN + 700)
        );
    }

    #[test]
    fn too_large_reported_once_and_again_on_redrop() {
        let mut buf = vec![0u8; 1024];
        let mut a = Assembler::new(&mut buf, limits());
        let image = app(3000, VARIANT_OLED);
        let bs = blocks(&image, DELUGE_OLED.app);
        let events: Vec<Event> = bs.iter().map(|b| a.write(b)).collect();
        let refusals = events
            .iter()
            .filter(|e| **e == Event::Refused(Refusal::TooLarge))
            .count();
        assert_eq!(refusals, 1);
        // Dropped again: block 0 reports it again.
        let again: Vec<Event> = bs.iter().map(|b| a.write(b)).collect();
        assert!(again.contains(&Event::Refused(Refusal::TooLarge)));
    }

    #[test]
    fn app_loading_outside_ram_is_damaged() {
        let body = vec![7u8; 600];
        let mut image = encode_app_header(&body, VARIANT_OLED, 0x2001_0000, 0x2001_0000).to_vec();
        image.extend_from_slice(&body);
        assert_eq!(
            validate(Target::App, &image, &checks(SLOT_FORM)),
            Err(Refusal::Damaged)
        );
    }

    #[test]
    fn only_the_image_s_own_bytes_are_written() {
        let mut image = app(700, VARIANT_OLED);
        image.extend_from_slice(&[0xEE; 300]);
        assert_eq!(
            validate(Target::App, &image, &checks(SLOT_FORM)),
            Ok(64 + 700)
        );
    }

    #[test]
    fn complete_file_is_kept_until_reset() {
        let a_img = app(1000, VARIANT_OLED);
        let b_img = app(2000, VARIANT_OLED);
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        assert!(matches!(
            feed(&mut a, &blocks(&a_img, DELUGE_OLED.app)),
            Event::Complete { .. }
        ));
        // More sectors in the same host write: another file's block.
        let b_bs = blocks(&b_img, DELUGE_OLED.app);
        assert_eq!(a.write(&b_bs[0]), Event::Ignored);
        assert_eq!(a.image(), &a_img[..]);
        a.reset();
        assert!(matches!(a.write(&b_bs[0]), Event::Progress { .. }));
    }

    #[test]
    fn same_shape_mix_is_never_written() {
        // An out-of-order host left file A's second half; file B, the same block count, fills
        // the gaps. The assembler can't tell the two apart; the body CRC can.
        let a_img = app(4000, VARIANT_OLED);
        let mut b_img = a_img.clone();
        b_img[100..3000].fill(0x42);
        let (a_bs, b_bs) = (
            blocks(&a_img, DELUGE_OLED.app),
            blocks(&b_img, DELUGE_OLED.app),
        );
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let half = a_bs.len() / 2;
        feed(&mut a, &a_bs[half..]);
        assert!(matches!(
            feed(&mut a, &b_bs[..half]),
            Event::Complete { .. }
        ));
        assert_eq!(
            validate(Target::App, a.image(), &checks(SLOT_FORM)),
            Err(Refusal::Damaged)
        );
    }

    #[test]
    fn in_order_redrop_of_another_build_completes_first_time() {
        // A cancelled in-order copy of A, then B in order: B overwrites from block 0.
        let a_img = app(4000, VARIANT_OLED);
        let mut b_img = a_img.clone();
        b_img[100..3000].fill(0x42);
        let (a_bs, b_bs) = (
            blocks(&a_img, DELUGE_OLED.app),
            blocks(&b_img, DELUGE_OLED.app),
        );
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        feed(&mut a, &a_bs[..a_bs.len() / 2]);
        assert!(matches!(feed(&mut a, &b_bs), Event::Complete { .. }));
        assert_eq!(a.image(), &b_img[..]);
    }

    #[test]
    fn huge_target_is_too_large() {
        let mut buf = vec![0u8; 0x2C_0000];
        let mut a = Assembler::new(&mut buf, limits());
        let b = crafted(DELUGE_OLED.app, 0, 2, u32::MAX - 10);
        assert_eq!(a.write(&b), Event::Refused(Refusal::TooLarge));
    }

    #[test]
    fn messages_are_the_spec_s() {
        assert_eq!(Refusal::WrongBoard.message(), b"WRONG BOARD");
        assert_eq!(Refusal::Damaged.message(), b"DAMAGED FILE");
        assert_eq!(Refusal::TooLarge.message(), b"TOO LARGE");
        assert_eq!(Refusal::NeedsSlotBoot.message(), b"NEEDS SLOT BOOT");
    }
}

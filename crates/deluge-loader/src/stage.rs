//! Where an app's segments go when some of them land on the running loader.
//!
//! Apps link where the loader itself runs, so a segment over the loader can't be written in
//! place: it is staged in a free window first, and a trampoline outside both copies it home and
//! branches to the app once the loader is done. [`staging_plan`] decides which segments are
//! staged and where, and refuses anything that would land on memory nobody may write (the boot
//! handoff word, the staging window itself).

use heapless::Vec;

/// A byte range `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u32,
    pub end: u32,
}

impl Region {
    fn overlaps(&self, o: &Region) -> bool {
        self.start < o.end && o.start < self.end
    }
}

/// A staged segment: the trampoline copies `len` bytes from `from` to `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub from: u32,
    pub to: u32,
    pub len: u32,
}

/// Why an app can't be staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageError {
    /// A segment lands on reserved memory or on the staging window.
    Reserved,
    /// The staged segments don't fit the staging window.
    StagingFull,
    /// More staged segments than [`MAX_MOVES`].
    TooMany,
    /// A flash app's body doesn't land wholly inside the RAM apps may load into.
    OutsideRam,
}

/// The most staged segments a plan holds.
pub const MAX_MOVES: usize = 16;

/// Plan the load of `segments` (each a destination range): a segment over `loader` is staged in
/// `staging`, in order, word-aligned; any other is written in place (no [`Move`]). A segment over
/// `reserved` or over `staging` is refused.
pub fn staging_plan(
    segments: &[Region],
    loader: Region,
    reserved: &[Region],
    staging: Region,
) -> Result<Vec<Move, MAX_MOVES>, StageError> {
    let mut moves = Vec::new();
    let mut next = u64::from(staging.start);
    for s in segments {
        if s.overlaps(&staging) || reserved.iter().any(|r| s.overlaps(r)) {
            return Err(StageError::Reserved);
        }
        if !s.overlaps(&loader) {
            continue;
        }
        let len = s.end - s.start;
        if next + u64::from(len) > u64::from(staging.end) {
            return Err(StageError::StagingFull);
        }
        moves
            .push(Move {
                from: next as u32,
                to: s.start,
                len,
            })
            .map_err(|_| StageError::TooMany)?;
        next = (next + u64::from(len) + 3) & !3;
    }
    Ok(moves)
}

/// The one copy that launches a flash app whose header (checked) sits at `header_at` in the
/// memory-mapped flash: its body, right after the header, to `load_addr`. Refused unless the body
/// lands wholly inside `ram`.
pub fn flash_app_move(
    h: &deluge_image::app::AppHeader,
    header_at: u32,
    ram: core::ops::Range<u32>,
) -> Result<Move, StageError> {
    let end = u64::from(h.load_addr) + u64::from(h.body_len);
    if h.load_addr < ram.start || end > u64::from(ram.end) {
        return Err(StageError::OutsideRam);
    }
    Ok(Move {
        from: header_at + deluge_image::app::APP_HEADER_LEN as u32,
        to: h.load_addr,
        len: h.body_len,
    })
}

/// The flash app slot's header when it's one the loader could launch: a valid DAPP header of
/// `variant` with a body of at most `max_body`, loading wholly inside `ram`. The body's CRC is
/// checked at launch; this is what the menu offers `BOOT FLASH` on.
pub fn bootable_header(
    header: &[u8],
    variant: u16,
    max_body: u32,
    ram: core::ops::Range<u32>,
) -> Option<deluge_image::app::AppHeader> {
    let h = deluge_image::app::validate_app_header(header, &[variant], max_body).ok()?;
    flash_app_move(&h, 0, ram).ok()?;
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_image::app::{APP_HEADER_LEN, AppHeader};

    const SRAM: core::ops::Range<u32> = 0x2002_0000..0x2030_0000;

    fn header(load_addr: u32, body_len: u32) -> AppHeader {
        AppHeader {
            variant: 0,
            load_addr,
            entry: load_addr,
            body_len,
            body_crc: 0,
        }
    }

    #[test]
    fn flash_app_move_inside_ram() {
        let m = flash_app_move(&header(0x2002_0000, 0x1000), 0x1810_0000, SRAM).unwrap();
        assert_eq!(
            m,
            Move {
                from: 0x1810_0000 + APP_HEADER_LEN as u32,
                to: 0x2002_0000,
                len: 0x1000
            }
        );
        // Exactly up to the end fits.
        assert!(flash_app_move(&header(0x2030_0000 - 0x1000, 0x1000), 0x1810_0000, SRAM).is_ok());
    }

    #[test]
    fn bootable_header_checks_where_it_loads() {
        use deluge_image::app::encode_app_header;
        let body = [0u8; 0x100];
        let inside = encode_app_header(&body, 0, 0x2002_0000, 0x2002_0000);
        let outside = encode_app_header(&body, 0, 0x2001_0000, 0x2001_0000);
        let h = bootable_header(&inside, 0, 0x1000, SRAM).expect("loads inside SRAM");
        assert_eq!(h.load_addr, 0x2002_0000);
        assert!(bootable_header(&outside, 0, 0x1000, SRAM).is_none());
        assert!(
            bootable_header(&inside, 1, 0x1000, SRAM).is_none(),
            "another variant"
        );
        assert!(
            bootable_header(&[0xFF; 64], 0, 0x1000, SRAM).is_none(),
            "erased"
        );
    }

    #[test]
    fn flash_app_move_outside_ram_refused() {
        assert_eq!(
            flash_app_move(&header(0x2001_0000, 0x1000), 0x1810_0000, SRAM),
            Err(StageError::OutsideRam)
        );
        assert_eq!(
            flash_app_move(&header(0x2030_0000 - 0x800, 0x1000), 0x1810_0000, SRAM),
            Err(StageError::OutsideRam)
        );
        // No wrap-around past 4 GiB.
        assert_eq!(
            flash_app_move(
                &header(0xFFFF_F000, 0x2000),
                0x1810_0000,
                0x2002_0000..u32::MAX
            ),
            Err(StageError::OutsideRam)
        );
    }

    const LOADER: Region = Region {
        start: 0x8002_0000,
        end: 0x8010_0000,
    };
    const RESERVED: [Region; 1] = [Region {
        start: 0x8000_0000,
        end: 0x8000_0100,
    }];
    const STAGING: Region = Region {
        start: 0x0C00_0000,
        end: 0x0C10_0000,
    };

    fn r(start: u32, len: u32) -> Region {
        Region {
            start,
            end: start + len,
        }
    }

    #[test]
    fn segments_clear_of_the_loader_go_direct() {
        let plan = staging_plan(&[r(0x8020_0000, 0x100)], LOADER, &RESERVED, STAGING).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn segments_over_the_loader_are_staged_in_order() {
        let plan = staging_plan(
            &[r(0x8002_0000, 0x1000), r(0x8003_0000, 0x10)],
            LOADER,
            &RESERVED,
            STAGING,
        )
        .unwrap();
        assert_eq!(
            plan.as_slice(),
            [
                Move {
                    from: 0x0C00_0000,
                    to: 0x8002_0000,
                    len: 0x1000
                },
                Move {
                    from: 0x0C00_1000,
                    to: 0x8003_0000,
                    len: 0x10
                },
            ]
        );
    }

    #[test]
    fn staging_plan_keeps_clear_of_reserved() {
        assert_eq!(
            staging_plan(&[r(0x8000_0080, 0x100)], LOADER, &RESERVED, STAGING),
            Err(StageError::Reserved)
        );
        // A segment landing in the staging window would overwrite what is staged there.
        assert_eq!(
            staging_plan(&[r(0x0C00_0000, 4)], LOADER, &RESERVED, STAGING),
            Err(StageError::Reserved)
        );
    }

    #[test]
    fn staging_overflow_refused() {
        assert_eq!(
            staging_plan(
                &[r(0x8002_0000, 0xE_0000)],
                LOADER,
                &RESERVED,
                r(0x0C00_0000, 0x1000)
            ),
            Err(StageError::StagingFull)
        );
    }

    #[test]
    fn staged_copies_are_word_aligned() {
        let plan = staging_plan(
            &[r(0x8002_0000, 3), r(0x8002_0100, 4)],
            LOADER,
            &RESERVED,
            STAGING,
        )
        .unwrap();
        assert_eq!(plan[1].from, 0x0C00_0004);
    }

    #[test]
    fn too_many_staged_segments_refused() {
        let segs: std::vec::Vec<Region> = (0..17).map(|i| r(0x8002_0000 + i * 0x100, 4)).collect();
        assert_eq!(
            staging_plan(&segs, LOADER, &RESERVED, STAGING),
            Err(StageError::TooMany)
        );
    }
}

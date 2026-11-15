//! The boot droplet's policy: whether a boot shows it static (with a marker saying why the menu is
//! coming) or animated, and how far a load has drawn it. The firmware's `splash` module draws it.

use deluge_image::settings::{BootMode, Settings, boot_mode};

/// Blocks in the whole droplet.
pub const LOGO_BLOCKS: u8 = 25;
/// Blocks an animated boot draws on a timer before a load takes over.
pub const PRE_ROLL_BLOCKS: u8 = 10;
/// Milliseconds between pre-roll blocks.
pub const PRE_ROLL_PERIOD_MS: u64 = 25;

/// Why a boot is headed for the menu, shown in a corner of the static droplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// Dev mode: `(D)`.
    Dev,
    /// LOAD held at power-on: `(R)`.
    Recovery,
}

impl Marker {
    /// The letter drawn between the parentheses.
    pub const fn glyph(self) -> u8 {
        match self {
            Marker::Dev => b'D',
            Marker::Recovery => b'R',
        }
    }
}

/// What the display shows from the moment it is up until the menu or a launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplashPlan {
    /// An INSTANT boot: the droplet builds up through the loader's work and the load.
    Animate,
    /// A boot headed for the menu: the whole droplet at once, with the marker if any.
    Static(Option<Marker>),
}

/// The splash for a boot with settings `cfg`, `recovery` when LOAD was held. Decided before the
/// card is listed, so it assumes something is bootable: a boot that turns out to have nothing
/// still reaches the menu, which replaces the droplet.
pub fn plan(cfg: &Settings, recovery: bool) -> SplashPlan {
    if boot_mode(cfg, 1, recovery, true) == BootMode::Instant {
        return SplashPlan::Animate;
    }
    let marker = if recovery {
        Some(Marker::Recovery)
    } else if cfg.dev_mode {
        Some(Marker::Dev)
    } else {
        None
    };
    SplashPlan::Static(marker)
}

/// Blocks to show `done` of `total` bytes into a load that started with `base` blocks drawn: the
/// rest of the droplet in proportion, the last block only once the load is complete. An empty
/// (`total == 0`) or overrun load is complete.
pub fn load_blocks(base: u8, done: u32, total: u32) -> u8 {
    let base = base.min(LOGO_BLOCKS);
    if total == 0 || done >= total {
        return LOGO_BLOCKS;
    }
    let rest = u64::from(LOGO_BLOCKS - base);
    base + (rest * u64::from(done) / u64::from(total)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_image::settings::AutoBoot;

    fn cfg(dev_mode: bool, auto_boot: AutoBoot) -> Settings {
        Settings { dev_mode, auto_boot }
    }

    #[test]
    fn instant_animates() {
        assert_eq!(plan(&cfg(false, AutoBoot::Instant), false), SplashPlan::Animate);
    }

    #[test]
    fn countdown_is_static_unmarked() {
        assert_eq!(plan(&cfg(false, AutoBoot::Secs(5)), false), SplashPlan::Static(None));
    }

    #[test]
    fn never_is_static_unmarked() {
        assert_eq!(plan(&cfg(false, AutoBoot::Never), false), SplashPlan::Static(None));
    }

    #[test]
    fn dev_mode_is_static_d() {
        assert_eq!(
            plan(&cfg(true, AutoBoot::Instant), false),
            SplashPlan::Static(Some(Marker::Dev))
        );
        assert_eq!(Marker::Dev.glyph(), b'D');
    }

    #[test]
    fn recovery_on_instant_is_static_r() {
        assert_eq!(
            plan(&cfg(false, AutoBoot::Instant), true),
            SplashPlan::Static(Some(Marker::Recovery))
        );
        assert_eq!(Marker::Recovery.glyph(), b'R');
    }

    #[test]
    fn recovery_beats_dev() {
        assert_eq!(
            plan(&cfg(true, AutoBoot::Secs(5)), true),
            SplashPlan::Static(Some(Marker::Recovery))
        );
    }

    #[test]
    fn load_blocks_endpoints() {
        assert_eq!(load_blocks(10, 0, 100), 10);
        assert_eq!(load_blocks(10, 100, 100), 25);
        assert_eq!(load_blocks(0, 50, 100), 12);
    }

    #[test]
    fn load_blocks_empty_or_overrun_completes() {
        assert_eq!(load_blocks(10, 0, 0), 25);
        assert_eq!(load_blocks(10, 200, 100), 25);
    }

    #[test]
    fn load_blocks_monotonic() {
        for total in [1u32, 7, 100, 65_536, 1_000_003] {
            let mut last = 0;
            for i in 0..=1000u64 {
                let done = (u64::from(total) * i / 1000) as u32;
                let b = load_blocks(10, done, total);
                assert!(b >= last && (10..=25).contains(&b), "total {total} done {done}");
                last = b;
            }
        }
        // No overflow at the top of the range.
        assert_eq!(load_blocks(0, u32::MAX - 1, u32::MAX), 24);
        assert_eq!(load_blocks(0, u32::MAX, u32::MAX), 25);
    }

    #[test]
    fn load_blocks_never_below_base_or_above_full() {
        assert_eq!(load_blocks(30, 0, 100), 25);
        assert_eq!(load_blocks(25, 1, 100), 25);
    }
}

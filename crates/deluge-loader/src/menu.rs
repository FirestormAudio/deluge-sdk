//! The boot menu's entries, in the order the loader shows them, and how many of them are boot
//! targets. How to boot (at once, after a countdown, or wait) is `deluge_image::settings`'s
//! `boot_mode`, which takes [`boot_targets`] of these.

pub use deluge_image::settings::{AutoBoot, BootMode, Settings, boot_mode};
use heapless::Vec;

/// The most SD apps the menu lists (`/APPS/`, in directory order).
pub const MAX_SD_APPS: usize = 32;
/// The most entries [`entries`] returns.
pub const MAX_ENTRIES: usize = MAX_SD_APPS + 4;

/// One menu line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// The flash app slot's image (`BOOT FLASH`), the default.
    FlashApp,
    /// The `n`th app in `/APPS/`.
    SdApp(usize),
    /// USB mass storage for the card.
    DataTransfer,
    /// Write the new loader image on the card into the other boot slot.
    UpdateLoader,
    /// The loader's settings.
    Settings,
}

/// The menu: the flash app (when the slot holds a valid one), then the SD apps, then
/// `DATA TRANSFER`, `UPDATE LOADER` (when offered) and `SETTINGS`. The first entry is the default.
pub fn entries(flash_valid: bool, sd_apps: usize, update_offered: bool) -> Vec<Entry, MAX_ENTRIES> {
    let mut v = Vec::new();
    // Within capacity by construction: at most 1 + MAX_SD_APPS + 3.
    let mut push = |e| {
        let _ = v.push(e);
    };
    if flash_valid {
        push(Entry::FlashApp);
    }
    (0..sd_apps.min(MAX_SD_APPS)).for_each(|i| push(Entry::SdApp(i)));
    push(Entry::DataTransfer);
    if update_offered {
        push(Entry::UpdateLoader);
    }
    push(Entry::Settings);
    v
}

/// The line the menu shows for `entry`; an SD app shows `sd_names[n]` (empty past the end).
pub fn label<'a>(entry: Entry, sd_names: &[&'a [u8]]) -> &'a [u8] {
    match entry {
        Entry::FlashApp => b"BOOT FLASH",
        Entry::SdApp(n) => sd_names.get(n).copied().unwrap_or(b""),
        Entry::DataTransfer => b"DATA TRANSFER",
        Entry::UpdateLoader => b"UPDATE LOADER",
        Entry::Settings => b"SETTINGS",
    }
}

/// How many of `entries` launch something (what `boot_mode` calls `boot_total`).
pub fn boot_targets(entries: &[Entry]) -> usize {
    entries
        .iter()
        .filter(|e| matches!(e, Entry::FlashApp | Entry::SdApp(_)))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_order_matches_today() {
        assert_eq!(
            entries(true, 2, false).as_slice(),
            [
                Entry::FlashApp,
                Entry::SdApp(0),
                Entry::SdApp(1),
                Entry::DataTransfer,
                Entry::Settings
            ]
        );
        assert_eq!(
            entries(false, 1, true).as_slice(),
            [
                Entry::SdApp(0),
                Entry::DataTransfer,
                Entry::UpdateLoader,
                Entry::Settings
            ]
        );
    }

    /// Today's labels, as the loader has always shown them; an SD app shows its own name.
    #[test]
    fn menu_strings_unchanged() {
        let names: [&[u8]; 1] = [b"MYAPP.ELF"];
        let label = |e| label(e, &names);
        assert_eq!(label(Entry::FlashApp), b"BOOT FLASH");
        assert_eq!(label(Entry::SdApp(0)), b"MYAPP.ELF");
        assert_eq!(label(Entry::DataTransfer), b"DATA TRANSFER");
        assert_eq!(label(Entry::UpdateLoader), b"UPDATE LOADER");
        assert_eq!(label(Entry::Settings), b"SETTINGS");
    }

    #[test]
    fn boot_targets_count_only_apps() {
        assert_eq!(boot_targets(&entries(true, 3, true)), 4);
        assert_eq!(boot_targets(&entries(false, 0, true)), 0);
    }

    #[test]
    fn sd_apps_are_capped() {
        let e = entries(true, 100, true);
        assert_eq!(
            e.iter().filter(|x| matches!(x, Entry::SdApp(_))).count(),
            MAX_SD_APPS
        );
        assert_eq!(e.last(), Some(&Entry::Settings));
    }

    /// LOAD held forces the menu, and no boot target never auto-launches: today's
    /// `deluge_image::settings::boot_mode` rules, through this menu's count.
    #[test]
    fn load_held_forces_menu() {
        let cfg = Settings {
            auto_boot: AutoBoot::Instant,
            ..Settings::default()
        };
        let e = entries(true, 0, false);
        assert_eq!(
            boot_mode(&cfg, boot_targets(&e), true, true),
            BootMode::Wait
        );
        assert_eq!(
            boot_mode(&cfg, boot_targets(&e), false, true),
            BootMode::Instant
        );
    }

    #[test]
    fn no_flash_app_never_autolaunches() {
        let cfg = Settings {
            auto_boot: AutoBoot::Instant,
            ..Settings::default()
        };
        let e = entries(false, 0, true);
        assert_eq!(
            boot_mode(&cfg, boot_targets(&e), false, true),
            BootMode::Wait
        );
    }
}

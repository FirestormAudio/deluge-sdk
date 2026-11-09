//! Enumerates loadable ELF application images from the `/APPS` directory on the SD card.
//!
//! All non-directory entries in `/APPS` except macOS `._*` metadata files are treated as
//! potential app images; the ELF parser rejects non-ELF files at load time.
//!
//! No heap allocation is used. Up to [`MAX_APPS`] entries are stored in a fixed-size list, each
//! with its full name (to open it) and a display label (the first [`DISPLAY_NAME_MAX`] bytes).

use core::cmp::Ordering;

use deluge::sd::{DirEntry, File};
use deluge::{FatError, Sd};

/// Maximum number of application images that can be listed.
pub const MAX_APPS: usize = 32;
/// Maximum bytes kept for display labels (UTF-8 bytes, truncated if longer).
pub const DISPLAY_NAME_MAX: usize = 64;
/// Maximum bytes of a name the list keeps to open it: a FAT long name's 255 UCS-2 units, each up
/// to 3 bytes of UTF-8.
pub const NAME_MAX: usize = 255 * 3;

/// The directory apps are listed from.
const APPS_DIR: &str = "APPS";

/// A fixed-capacity list of the apps in `/APPS`, sorted by name.
pub struct EntryList {
    names: [[u8; NAME_MAX]; MAX_APPS],
    lens: [usize; MAX_APPS],
    count: usize,
}

impl EntryList {
    fn new() -> Self {
        Self {
            names: [[0u8; NAME_MAX]; MAX_APPS],
            lens: [0usize; MAX_APPS],
            count: 0,
        }
    }

    fn push(&mut self, name: &str) {
        let bytes = name.as_bytes();
        if self.count < MAX_APPS && bytes.len() <= NAME_MAX {
            self.names[self.count][..bytes.len()].copy_from_slice(bytes);
            self.lens[self.count] = bytes.len();
            self.count += 1;
        }
    }

    /// Number of entries found.
    pub fn len(&self) -> usize {
        self.count
    }

    /// The full name of entry `idx`. Panics if `idx >= len()`.
    pub fn name(&self, idx: usize) -> &str {
        assert!(idx < self.count);
        // Only whole `&str`s were pushed.
        core::str::from_utf8(&self.names[idx][..self.lens[idx]]).unwrap_or("")
    }

    /// Display label for entry `idx`: its name, cut to [`DISPLAY_NAME_MAX`] bytes.
    pub fn display_name(&self, idx: usize) -> &[u8] {
        let n = self.lens[idx].min(DISPLAY_NAME_MAX);
        &self.names[idx][..n]
    }

    /// Sort entries alphabetically by name.
    fn sort(&mut self) {
        // Insertion sort — fine for up to MAX_APPS entries.
        for i in 1..self.count {
            let mut j = i;
            while j > 0 {
                let a = &self.names[j - 1][..self.lens[j - 1]];
                let b = &self.names[j][..self.lens[j]];
                if cmp_display_names(a, b) == Ordering::Greater {
                    self.names.swap(j - 1, j);
                    self.lens.swap(j - 1, j);
                    j -= 1;
                } else {
                    break;
                }
            }
        }
    }
}

fn cmp_ascii_case_insensitive(a: &[u8], b: &[u8]) -> Ordering {
    let n = core::cmp::min(a.len(), b.len());
    for i in 0..n {
        let ac = a[i].to_ascii_lowercase();
        let bc = b[i].to_ascii_lowercase();
        match ac.cmp(&bc) {
            Ordering::Equal => {}
            non_eq => return non_eq,
        }
    }
    a.len().cmp(&b.len())
}

fn cmp_display_names(a: &[u8], b: &[u8]) -> Ordering {
    if let (Ok(a_str), Ok(b_str)) = (core::str::from_utf8(a), core::str::from_utf8(b)) {
        // Unicode-aware case-insensitive compare for valid UTF-8 LFN labels.
        let folded = a_str
            .chars()
            .flat_map(|c| c.to_lowercase())
            .cmp(b_str.chars().flat_map(|c| c.to_lowercase()));
        if folded != Ordering::Equal {
            return folded;
        }
        // Tie-break on original form to keep sort deterministic.
        return a_str.cmp(b_str);
    }

    // Fallback for short names or truncated non-UTF8 labels.
    let folded = cmp_ascii_case_insensitive(a, b);
    if folded != Ordering::Equal {
        return folded;
    }
    a.cmp(b)
}

/// List `/APPS`, sorted. An empty list (not an error) if `/APPS` does not exist.
pub async fn list_apps(sd: &mut Sd) -> Result<EntryList, FatError> {
    let mut list = EntryList::new();
    let listed = sd
        .list(APPS_DIR, |e: &DirEntry<'_>| {
            // A `?` can't be in a FAT name: the SDK shows a non-ASCII short name (a file with no
            // long name, written by an old tool) that way, and no name opens it, so it's left out.
            if e.name.contains('?') {
                log::warn!("/APPS: {} has no name the loader can open; skipped", e.name);
            } else if !e.is_dir && !e.name.starts_with("._") {
                list.push(e.name);
            }
        })
        .await;
    match listed {
        Ok(()) | Err(FatError::NotFound) => {}
        Err(e) => return Err(e),
    }
    list.sort();
    Ok(list)
}

/// Open app `idx` of `list` for reading.
pub async fn open_app<'a>(
    sd: &'a mut Sd,
    list: &EntryList,
    idx: usize,
) -> Result<File<'a>, FatError> {
    sd.open_in(APPS_DIR, list.name(idx)).await
}

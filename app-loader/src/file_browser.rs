//! Enumerates loadable ELF application images from the `/APPS` directory on
//! the SD card's first FAT volume.
//!
//! All non-directory entries in `/APPS` are treated as potential app images;
//! the ELF parser will reject non-ELF files at load time.
//!
//! No heap allocation is used. Up to [`MAX_APPS`] entries are stored in a
//! fixed-size stack-allocated list.

use core::cmp::Ordering;
use deluge_bsp::fat::{
    self, DirEntry, FatError, LfnBuffer, Mode, RawDirectory, RawFile, RawVolume, VolumeIdx,
};

/// Maximum number of application images that can be listed.
pub const MAX_APPS: usize = 32;
/// Maximum bytes kept for display labels (UTF-8 bytes, truncated if longer).
pub const DISPLAY_NAME_MAX: usize = 64;

/// A fixed-capacity list of directory entries collected from `/APPS`.
pub struct EntryList {
    entries: [core::mem::MaybeUninit<DirEntry>; MAX_APPS],
    display_names: [[u8; DISPLAY_NAME_MAX]; MAX_APPS],
    display_lens: [usize; MAX_APPS],
    count: usize,
}

impl EntryList {
    fn new() -> Self {
        Self {
            entries: core::array::from_fn(|_| core::mem::MaybeUninit::uninit()),
            display_names: [[0u8; DISPLAY_NAME_MAX]; MAX_APPS],
            display_lens: [0usize; MAX_APPS],
            count: 0,
        }
    }

    fn push(&mut self, e: DirEntry, lfn: Option<&str>) {
        if self.count < MAX_APPS {
            self.display_lens[self.count] =
                make_display_name(&e, lfn, &mut self.display_names[self.count]);
            self.entries[self.count].write(e);
            self.count += 1;
        }
    }

    /// Number of entries found.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Reference to the entry at `idx`. Panics if `idx >= len()`.
    pub fn get(&self, idx: usize) -> &DirEntry {
        assert!(idx < self.count);
        // SAFETY: entries[0..count] were initialised by `push`.
        unsafe { self.entries[idx].assume_init_ref() }
    }

    /// Display label for entry `idx` (LFN when present, otherwise short name).
    pub fn display_name(&self, idx: usize) -> &[u8] {
        let n = self.display_lens[idx];
        &self.display_names[idx][..n]
    }

    /// Sort entries alphabetically by display label.
    fn sort(&mut self) {
        // Insertion sort — fine for up to MAX_APPS entries.
        for i in 1..self.count {
            let mut j = i;
            while j > 0 {
                let a = {
                    let n = self.display_lens[j - 1];
                    &self.display_names[j - 1][..n]
                };
                let b = {
                    let n = self.display_lens[j];
                    &self.display_names[j][..n]
                };
                if cmp_display_names(a, b) == Ordering::Greater {
                    // Swap MaybeUninit slots.
                    self.entries.swap(j - 1, j);
                    self.display_names.swap(j - 1, j);
                    self.display_lens.swap(j - 1, j);
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

impl Drop for EntryList {
    fn drop(&mut self) {
        // Drop initialised entries manually since MaybeUninit doesn't auto-drop.
        for i in 0..self.count {
            unsafe { self.entries[i].assume_init_drop() };
        }
    }
}

fn write_short_name(entry: &DirEntry, out: &mut [u8]) -> usize {
    let name = &entry.name;
    let base = name.base_name();
    let ext = name.extension();
    let mut n = 0;

    for &b in base {
        if n >= out.len() {
            return n;
        }
        out[n] = b;
        n += 1;
    }
    if !ext.is_empty() {
        if n < out.len() {
            out[n] = b'.';
            n += 1;
        }
        for &b in ext {
            if n >= out.len() {
                break;
            }
            out[n] = b;
            n += 1;
        }
    }
    n
}

fn make_display_name(
    entry: &DirEntry,
    lfn: Option<&str>,
    out: &mut [u8; DISPLAY_NAME_MAX],
) -> usize {
    if let Some(name) = lfn {
        let bytes = name.as_bytes();
        if !bytes.is_empty() {
            let n = core::cmp::min(bytes.len(), out.len());
            out[..n].copy_from_slice(&bytes[..n]);
            return n;
        }
    }
    write_short_name(entry, out)
}

/// Browse `/APPS` and return a sorted `EntryList` plus the open volume and
/// root directory handles (caller must close them after loading).
///
/// An empty list is returned (not an error) if `/APPS` does not exist or
/// contains no files.
pub fn list_apps(
    vm: &mut fat::DelugeVolumeManager,
) -> Result<(RawVolume, RawDirectory, EntryList), FatError> {
    let volume = vm.open_raw_volume(VolumeIdx(0))?;
    let root = vm.open_root_dir(volume)?;

    let apps_dir = match vm.open_dir(root, "APPS") {
        Ok(dir) => dir,
        Err(_) => {
            // /APPS doesn't exist — return with empty list so the caller can
            // display an appropriate error.
            return Ok((volume, root, EntryList::new()));
        }
    };

    let mut list = EntryList::new();

    let mut lfn_storage = [0u8; 260];
    let mut lfn_buf = LfnBuffer::new(&mut lfn_storage);
    vm.iterate_dir_lfn(apps_dir, &mut lfn_buf, |e, lfn| {
        if e.attributes.is_directory() {
            return;
        }
        let is_mac_hidden = lfn
            .map(|n| n.starts_with("._"))
            .unwrap_or_else(|| e.name.base_name().starts_with(b"._"));
        if !is_mac_hidden {
            list.push(e.clone(), lfn);
        }
    })?;

    vm.close_dir(apps_dir)?;
    list.sort();

    Ok((volume, root, list))
}

/// Open an app entry for reading and return the `RawFile` handle.
///
/// The caller must close the file with `vm.close_file(file)` when done.
pub fn open_app(
    vm: &mut fat::DelugeVolumeManager,
    root: RawDirectory,
    entry: &DirEntry,
) -> Result<RawFile, FatError> {
    // Re-open via the APPS subdir (we stored root, not apps_dir).
    let apps_dir = vm.open_dir(root, "APPS")?;
    let file = match vm.open_file_in_dir(apps_dir, &entry.name, Mode::ReadOnly) {
        Ok(f) => f,
        Err(e) => {
            let _ = vm.close_dir(apps_dir);
            return Err(e);
        }
    };
    vm.close_dir(apps_dir)?;
    Ok(file)
}

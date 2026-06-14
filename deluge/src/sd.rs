//! SD-card file access (FAT).

use deluge_bsp::fat::{self, FatError, Mode, VolumeIdx};

/// SD-card files, taken once from [`Deluge::sd`](crate::Deluge::sd).
///
/// A small convenience API for whole-file reads/writes of files in the card's
/// **root** directory (the common maker need: a config/preset/sample file). For
/// directory trees or streaming, drop down to [`deluge_bsp::fat`] /
/// [`embedded_sdmmc`].
///
/// Each call mounts the FAT volume, does the operation, and unmounts — so handles
/// never leak across calls. The card hardware is initialised once when the handle
/// is taken.
pub struct Sd {
    _private: (),
}

impl Sd {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Read a root-directory file into `buf`; returns the number of bytes read
    /// (capped at `buf.len()`).
    pub fn read(&mut self, name: &str, buf: &mut [u8]) -> Result<usize, FatError> {
        let vm = fat::new_volume_manager();
        let volume = vm.open_raw_volume(VolumeIdx(0))?;
        let root = vm.open_root_dir(volume)?;
        let file = vm.open_file_in_dir(root, name, Mode::ReadOnly)?;

        let mut total = 0;
        while total < buf.len() {
            let n = vm.read(file, &mut buf[total..])?;
            if n == 0 {
                break;
            }
            total += n;
        }

        vm.close_file(file)?;
        // Dropping `vm` releases the volume/dir handles.
        Ok(total)
    }

    /// Write `data` to a root-directory file, creating or truncating it.
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), FatError> {
        let vm = fat::new_volume_manager();
        let volume = vm.open_raw_volume(VolumeIdx(0))?;
        let root = vm.open_root_dir(volume)?;
        let file = vm.open_file_in_dir(root, name, Mode::ReadWriteCreateOrTruncate)?;

        vm.write(file, data)?;
        // close_file flushes; must happen before `vm` is dropped.
        vm.close_file(file)?;
        Ok(())
    }
}

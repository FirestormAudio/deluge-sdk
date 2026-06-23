//! SD-card file access (FAT).

#[cfg(target_os = "none")]
use deluge_bsp::fat::{self, FatError, Mode, VolumeIdx};

/// Initialise the SD card. On the device this powers up and probes the card; on
/// the host simulator the "card" is a local directory, so there is nothing to do.
#[cfg(target_os = "none")]
pub(crate) async fn init_card() -> Result<(), deluge_bsp::sd::SdError> {
    deluge_bsp::sd::init().await
}
/// Host: the simulated card is always available.
#[cfg(not(target_os = "none"))]
pub(crate) async fn init_card() -> Result<(), SdError> {
    Ok(())
}

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
///
/// On the host simulator the "card root" is a local directory (the
/// `DELUGE_SIM_SD` env var, default `./sim-sd`), so reads/writes hit real files.
pub struct Sd {
    _private: (),
}

#[cfg(target_os = "none")]
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

// ── Host (desktop simulator) ─────────────────────────────────────────────────

/// Host SD-card hardware error (mirrors [`deluge_bsp::sd::SdError`]'s role).
#[cfg(not(target_os = "none"))]
#[derive(Debug)]
#[non_exhaustive]
pub enum SdError {
    /// I/O error talking to the simulated card directory.
    Io,
}

/// Host filesystem error from [`Sd`] read/write.
#[cfg(not(target_os = "none"))]
#[derive(Debug)]
#[non_exhaustive]
pub enum FatError {
    /// The file does not exist.
    NotFound,
    /// Other I/O error.
    Io,
}

#[cfg(not(target_os = "none"))]
impl From<std::io::Error> for FatError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => FatError::NotFound,
            _ => FatError::Io,
        }
    }
}

/// The simulated SD-card root directory (`DELUGE_SIM_SD`, default `./sim-sd`).
#[cfg(not(target_os = "none"))]
fn sim_sd_root() -> std::path::PathBuf {
    std::env::var_os("DELUGE_SIM_SD")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("sim-sd"))
}

#[cfg(not(target_os = "none"))]
impl Sd {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Read a root-directory file into `buf`; returns the number of bytes read.
    pub fn read(&mut self, name: &str, buf: &mut [u8]) -> Result<usize, FatError> {
        let data = std::fs::read(sim_sd_root().join(name))?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }

    /// Write `data` to a root-directory file, creating or truncating it.
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), FatError> {
        let root = sim_sd_root();
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join(name), data)?;
        Ok(())
    }
}

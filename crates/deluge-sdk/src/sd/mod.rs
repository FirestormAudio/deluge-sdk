//! SD-card files (FAT), the same API on every board.

mod error;
#[cfg(any(target_os = "none", test))]
pub(crate) mod fat;

pub use error::{FatError, SdError};

/// A 32-byte aligned buffer. A [`File`] read into one whose length is a multiple of 512 goes
/// straight to the card in multi-block transfers, full speed on every board, as long as the file
/// position is a multiple of 32 too: after a [`File::seek`] to an offset that isn't, the reads
/// that follow go a block at a time.
#[repr(C, align(32))]
pub struct AlignedBuf<const N: usize>(pub [u8; N]);

/// SD-card files, taken once from [`Deluge::sd`](crate::Deluge::sd).
///
/// Files in the card's **root** directory, by name (long names work): whole-file
/// [`read`](Self::read) and [`write`](Self::write), and [`open`](Self::open) for a [`File`] read
/// a chunk at a time, with seek. Every call awaits the card, so the rest of the app keeps running
/// while a transfer waits on the DMA.
///
/// **Let every call finish.** Don't drop an `Sd` or [`File`] future before it completes (no
/// `select` or timeout around one): a card transfer cut off mid-way leaves the card unusable
/// until the next power-up, every later call failing with [`FatError::Card`], and a cut-off
/// [`write`](Self::write) can leave its file inconsistent.
///
/// On the device the card's FAT volume is mounted once, when the handle is taken. On the host
/// simulator the "card root" is a local directory (the `DELUGE_SIM_SD` env var, default
/// `./sim-sd`), so reads and writes hit real files. The Linux backend does not implement SD
/// access yet and panics.
pub struct Sd {
    card: crate::plat::SdCard,
    _not_send: crate::NotSend,
}

/// `name` as a root-directory file name: not empty, not `.` or `..`, no path separator. Every
/// backend checks it here, so none reaches a subdirectory (or, in the simulator, a host path).
fn root_name(name: &str) -> Result<&str, FatError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(FatError::InvalidName);
    }
    Ok(name)
}

impl Sd {
    /// Bring the card up and mount its volume.
    pub(crate) async fn mount() -> Result<Self, SdError> {
        Ok(Self {
            card: crate::plat::sd_mount().await?,
            _not_send: crate::NOT_SEND,
        })
    }

    /// Read a root-directory file into `buf`: the bytes read, at most `buf.len()`.
    pub async fn read(&mut self, name: &str, buf: &mut [u8]) -> Result<usize, FatError> {
        self.card.read(root_name(name)?, buf).await
    }

    /// Write `data` to a root-directory file, creating or truncating it. `Ok` means it is on the
    /// card. A write that fails (the card full, a transfer error) leaves the file empty.
    pub async fn write(&mut self, name: &str, data: &[u8]) -> Result<(), FatError> {
        self.card.write(root_name(name)?, data).await
    }

    /// Open a root-directory file to read a chunk at a time. One file is open at a time: the
    /// [`File`] borrows the `Sd`.
    pub async fn open(&mut self, name: &str) -> Result<File<'_>, FatError> {
        Ok(File {
            file: self.card.open(root_name(name)?).await?,
            _not_send: crate::NOT_SEND,
        })
    }
}

/// A root-directory file open to read, from [`Sd::open`].
pub struct File<'a> {
    file: crate::plat::SdFile<'a>,
    _not_send: crate::NotSend,
}

impl File<'_> {
    /// The file's length in bytes.
    #[expect(
        clippy::len_without_is_empty,
        reason = "a file's length, not a collection's"
    )]
    pub fn len(&self) -> u32 {
        self.file.len()
    }

    /// Read on from where the file is: fills `buf` unless the file ends first, and returns the
    /// bytes read (0 at the end). Into an [`AlignedBuf`] a multiple of 512 long, from a position
    /// that is a multiple of 32, at full speed.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, FatError> {
        self.file.read(buf).await
    }

    /// Move to `offset` bytes from the start. `offset` may be [`len`](Self::len), the end;
    /// past it is [`FatError::SeekPastEnd`].
    pub async fn seek(&mut self, offset: u32) -> Result<(), FatError> {
        self.file.seek(offset).await
    }

    /// Close the file.
    pub async fn close(self) -> Result<(), FatError> {
        self.file.close().await
    }
}

#[cfg(all(test, feature = "sim"))]
mod sim_tests {
    extern crate std;

    use std::sync::Mutex;
    use std::vec::Vec;

    use embassy_futures::block_on;

    use super::*;

    /// `DELUGE_SIM_SD` is process-wide: one test at a time.
    static CARD_DIR: Mutex<()> = Mutex::new(());

    /// Run `f` on an `Sd` over a fresh, empty simulated card.
    fn with_card(test: &str, f: impl AsyncFnOnce(&mut Sd)) {
        let _turn = CARD_DIR.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(std::format!("deluge-sim-sd-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: tests that touch the environment hold `CARD_DIR`.
        unsafe { std::env::set_var("DELUGE_SIM_SD", &dir) };
        block_on(async {
            let mut sd = Sd::mount().await.unwrap();
            f(&mut sd).await;
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sim_names_outside_the_root_are_invalid() {
        with_card("names", async |sd| {
            for name in ["", ".", "..", "../x", "a/b.txt", "a\\b.txt", "/x"] {
                assert_eq!(
                    sd.write(name, b"x").await,
                    Err(FatError::InvalidName),
                    "{name:?}"
                );
                let mut buf = [0; 4];
                assert_eq!(
                    sd.read(name, &mut buf).await,
                    Err(FatError::InvalidName),
                    "{name:?}"
                );
                assert!(
                    matches!(sd.open(name).await, Err(FatError::InvalidName)),
                    "{name:?}"
                );
            }
        });
    }

    #[test]
    fn sim_directory_name_is_not_found() {
        with_card("dir", async |sd| {
            let dir = std::path::PathBuf::from(std::env::var_os("DELUGE_SIM_SD").unwrap());
            std::fs::create_dir(dir.join("SAMPLES")).unwrap();
            let mut buf = [0; 4];
            assert_eq!(sd.read("SAMPLES", &mut buf).await, Err(FatError::NotFound));
            assert!(matches!(sd.open("SAMPLES").await, Err(FatError::NotFound)));
            assert_eq!(sd.write("SAMPLES", b"x").await, Err(FatError::InvalidName));
        });
    }

    #[test]
    fn sim_empty_file_and_the_end() {
        with_card("empty", async |sd| {
            sd.write("E.BIN", b"").await.unwrap();
            let mut f = sd.open("E.BIN").await.unwrap();
            assert_eq!(f.len(), 0);
            let mut buf = [0; 8];
            assert_eq!(f.read(&mut buf).await, Ok(0));
            f.close().await.unwrap();
            sd.write("F.BIN", b"abc").await.unwrap();
            let mut f = sd.open("F.BIN").await.unwrap();
            f.seek(3).await.unwrap();
            assert_eq!(f.read(&mut buf).await, Ok(0));
        });
    }

    #[test]
    fn sim_round_trip() {
        with_card("round-trip", async |sd| {
            sd.write("SDKTEST.TXT", b"deluge sdk").await.unwrap();
            let mut buf = [0; 32];
            assert_eq!(sd.read("SDKTEST.TXT", &mut buf).await, Ok(10));
            assert_eq!(&buf[..10], b"deluge sdk");
        });
    }

    #[test]
    fn sim_missing_file_is_not_found() {
        with_card("missing", async |sd| {
            let mut buf = [0; 4];
            assert_eq!(sd.read("NOPE.BIN", &mut buf).await, Err(FatError::NotFound));
            assert!(matches!(sd.open("NOPE.BIN").await, Err(FatError::NotFound)));
        });
    }

    #[test]
    fn sim_file_streams_and_seeks() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 7) as u8).collect();
        with_card("stream", async |sd| {
            sd.write("stream.bin", &data).await.unwrap();
            let mut f = sd.open("stream.bin").await.unwrap();
            assert_eq!(f.len(), 100_000);
            let mut got = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = f.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            assert_eq!(got, data);
            f.seek(4).await.unwrap();
            let mut four = [0; 4];
            assert_eq!(f.read(&mut four).await, Ok(4));
            assert_eq!(four, [28, 35, 42, 49]);
            assert_eq!(f.seek(100_001).await, Err(FatError::SeekPastEnd));
            f.close().await.unwrap();
        });
    }
}

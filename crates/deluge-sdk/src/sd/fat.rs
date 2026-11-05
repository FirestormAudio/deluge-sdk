//! The FAT volume behind [`Sd`](crate::Sd) on the device, the same on every board: embedded-fatfs
//! over `block-device-adapters`' byte stream over the board's 512-byte block device. Every card
//! transfer is awaited, so the rest of the system runs while one waits on the DMA.

use aligned::Aligned;
use block_device_adapters::{BufStream, StreamSlice};
use block_device_driver::BlockDevice;
use embedded_fatfs::{DefaultTimeProvider, Error, FileSystem, FsOptions, LossyOemCpConverter};
use embedded_io_async::{Read, Seek, SeekFrom, Write};

use super::error::FatError;

/// What the filesystem reads and writes: the volume's window of the card, a block at a time.
type Storage<D> = StreamSlice<BufStream<D, 512>>;
type Fs<D> = FileSystem<Storage<D>, DefaultTimeProvider, LossyOemCpConverter>;

/// A FAT volume, mounted.
pub(crate) struct Volume<D: BlockDevice<512>> {
    fs: Fs<D>,
}

/// An open file on a [`Volume`].
pub(crate) struct FatFile<'a, D: BlockDevice<512>> {
    file: embedded_fatfs::File<'a, Storage<D>, DefaultTimeProvider, LossyOemCpConverter>,
    len: u32,
}

impl<E> From<Error<E>> for FatError {
    fn from(e: Error<E>) -> Self {
        match e {
            Error::NotFound => FatError::NotFound,
            Error::NotEnoughSpace => FatError::Full,
            Error::InvalidFileNameLength | Error::UnsupportedFileNameCharacter => {
                FatError::InvalidName
            }
            // A name that is a directory's (or names no plain root file).
            Error::AlreadyExists | Error::InvalidInput | Error::DirectoryIsNotEmpty => {
                FatError::InvalidName
            }
            Error::Io(_) | Error::UnexpectedEof | Error::WriteZero => FatError::Card,
            // `CorruptedFileSystem`, and any variant a later embedded-fatfs adds.
            _ => FatError::NotFat,
        }
    }
}

/// Whether sector 0 is a FAT boot sector (a partitionless "superfloppy"): a jump, then a BPB with
/// 512-byte sectors and a power-of-two cluster size. An MBR's boot code can start with a jump too
/// (GRUB's does), so the jump alone doesn't tell.
fn is_boot_sector(s: &[u8]) -> bool {
    let jump = s[0] == 0xEB || s[0] == 0xE9;
    let bytes_per_sector = u16::from_le_bytes([s[0x0B], s[0x0C]]);
    jump && bytes_per_sector == 512 && s[0x0D].is_power_of_two()
}

/// The FAT volume's byte window on the card: partition 0 of an MBR, or the whole card for a
/// superfloppy ([`is_boot_sector`]).
async fn partition_window<D: BlockDevice<512>>(dev: &mut D) -> Result<(u64, u64), FatError> {
    let mut s0: [Aligned<D::Align, [u8; 512]>; 1] = [Aligned([0; 512])];
    dev.read(0, &mut s0).await.map_err(|_| FatError::Card)?;
    let s = &s0[0][..];
    if s[510..512] != [0x55, 0xAA] {
        return Err(FatError::NotFat);
    }
    if is_boot_sector(s) {
        let size = dev.size().await.map_err(|_| FatError::Card)?;
        return Ok((0, size));
    }
    let word = |at: usize| u32::from_le_bytes([s[at], s[at + 1], s[at + 2], s[at + 3]]) as u64;
    let (start, count) = (word(0x1BE + 8), word(0x1BE + 12));
    if start == 0 || count == 0 {
        return Err(FatError::NotFat);
    }
    Ok((start * 512, (start + count) * 512))
}

impl<D: BlockDevice<512>> Volume<D> {
    /// Mount the card's FAT volume.
    pub async fn mount(mut dev: D) -> Result<Self, FatError> {
        let (start, end) = partition_window(&mut dev).await?;
        let slice = StreamSlice::new(BufStream::new(dev), start, end)
            .await
            .map_err(|_| FatError::Card)?;
        let fs = FileSystem::new(slice, FsOptions::new()).await?;
        Ok(Self { fs })
    }

    /// Read a root-directory file into `buf`: the bytes read, at most `buf.len()`.
    pub async fn read(&self, name: &str, buf: &mut [u8]) -> Result<usize, FatError> {
        let mut file = self.open(name).await?;
        let n = file.read(buf).await?;
        file.close().await?;
        Ok(n)
    }

    /// Write `data` to a root-directory file, creating or truncating it. `Ok` means it is on the
    /// card: the file is flushed and closed, and the filesystem flushed. A write that fails (the
    /// card full, a transfer error) leaves the file empty, its clusters free again.
    pub async fn write(&self, name: &str, data: &[u8]) -> Result<(), FatError> {
        let mut file = self.fs.root_dir().create_file(name).await?;
        let written = async {
            file.truncate().await?;
            file.write_all(data).await?;
            file.flush().await
        }
        .await;
        if written.is_err() {
            // `truncate` freed the old clusters at once, and the directory entry is only written
            // on close: empty the file rather than close it over half its clusters.
            let _ = file.seek(SeekFrom::Start(0)).await;
            let _ = file.truncate().await;
        }
        // Closed and flushed whatever happened, so the directory entry, the FAT and the volume
        // agree; the first error is the one returned.
        let closed = file.close().await;
        let flushed = self.fs.flush().await;
        written?;
        closed?;
        flushed?;
        Ok(())
    }

    /// Open a root-directory file to read.
    pub async fn open(&self, name: &str) -> Result<FatFile<'_, D>, FatError> {
        // The entry, not `open_file`: it carries the length, and a directory isn't a file.
        let entry = self.fs.root_dir().open_meta(name).await?;
        if entry.is_dir() {
            return Err(FatError::NotFound);
        }
        let len = u32::try_from(entry.len()).unwrap_or(u32::MAX);
        Ok(FatFile {
            file: entry.to_file(),
            len,
        })
    }
}

impl<D: BlockDevice<512>> FatFile<'_, D> {
    /// The file's length in bytes.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Fill `buf` unless the file ends first: the bytes read, 0 at the end. A `buf` 32-byte
    /// aligned and a multiple of 512 long reaches the card in multi-block transfers.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, FatError> {
        let mut n = 0;
        while n < buf.len() {
            match self.file.read(&mut buf[n..]).await? {
                0 => break,
                got => n += got,
            }
        }
        Ok(n)
    }

    /// Move to `offset` bytes from the start; past the end is [`FatError::SeekPastEnd`].
    pub async fn seek(&mut self, offset: u32) -> Result<(), FatError> {
        if offset > self.len {
            return Err(FatError::SeekPastEnd);
        }
        self.file.seek(SeekFrom::Start(offset.into())).await?;
        Ok(())
    }

    /// Close the file.
    pub async fn close(self) -> Result<(), FatError> {
        self.file.close().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::vec;
    use std::vec::Vec;

    use aligned::{A4, Aligned};
    use embassy_futures::block_on;
    use embedded_fatfs::{FatType, FormatVolumeOptions, format_volume};

    use super::*;
    use crate::sd::AlignedBuf;

    /// A card in memory. Clones share the blocks, so a test can mount it again.
    #[derive(Clone)]
    struct RamDisk {
        blocks: Rc<RefCell<Vec<[u8; 512]>>>,
        fail_reads: Rc<Cell<bool>>,
        max_blocks_per_read: Rc<Cell<usize>>,
    }

    #[derive(Debug)]
    struct Failed;

    impl BlockDevice<512> for RamDisk {
        type Error = Failed;
        type Align = A4;

        async fn read(
            &mut self,
            at: u32,
            data: &mut [Aligned<A4, [u8; 512]>],
        ) -> Result<(), Failed> {
            if self.fail_reads.get() {
                return Err(Failed);
            }
            let max = self.max_blocks_per_read.get().max(data.len());
            self.max_blocks_per_read.set(max);
            let blocks = self.blocks.borrow();
            for (i, b) in data.iter_mut().enumerate() {
                b.copy_from_slice(&blocks[at as usize + i]);
            }
            Ok(())
        }

        async fn write(&mut self, at: u32, data: &[Aligned<A4, [u8; 512]>]) -> Result<(), Failed> {
            let mut blocks = self.blocks.borrow_mut();
            for (i, b) in data.iter().enumerate() {
                blocks[at as usize + i].copy_from_slice(&b[..]);
            }
            Ok(())
        }

        async fn size(&mut self) -> Result<u64, Failed> {
            Ok(self.blocks.borrow().len() as u64 * 512)
        }
    }

    /// The MBR partition's first sector.
    const PART_START: u32 = 2048;

    fn blank(mib: usize) -> RamDisk {
        RamDisk {
            blocks: Rc::new(RefCell::new(vec![[0; 512]; mib << 11])),
            fail_reads: Rc::new(Cell::new(false)),
            max_blocks_per_read: Rc::new(Cell::new(0)),
        }
    }

    /// A `mib` MiB card formatted with `options`: in an MBR's partition 0, or whole.
    fn format(mib: usize, mbr: bool, options: FormatVolumeOptions) -> RamDisk {
        let disk = blank(mib);
        let total = disk.blocks.borrow().len() as u32;
        let (start, count) = if mbr {
            (PART_START, total - PART_START)
        } else {
            (0, total)
        };
        block_on(async {
            let mut window = StreamSlice::new(
                BufStream::<_, 512>::new(disk.clone()),
                start as u64 * 512,
                (start + count) as u64 * 512,
            )
            .await
            .unwrap();
            format_volume(&mut window, options).await.unwrap();
            window.flush().await.unwrap();
        });
        if mbr {
            let mut blocks = disk.blocks.borrow_mut();
            let s0 = &mut blocks[0];
            s0[0x1BE + 4] = 0x0E; // FAT16 LBA
            s0[0x1BE + 8..0x1BE + 12].copy_from_slice(&start.to_le_bytes());
            s0[0x1BE + 12..0x1BE + 16].copy_from_slice(&count.to_le_bytes());
            s0[510] = 0x55;
            s0[511] = 0xAA;
        }
        disk
    }

    /// A 16 MiB card formatted FAT with 32 KiB clusters (FAT12): in an MBR's partition 0, or
    /// whole.
    fn formatted(mbr: bool) -> RamDisk {
        format(16, mbr, FormatVolumeOptions::new().bytes_per_cluster(32768))
    }

    /// A 64 MiB card, MBR partition 0 formatted FAT32 (512-byte clusters), as most SDHC cards
    /// are FAT32.
    fn formatted_fat32() -> RamDisk {
        let options = FormatVolumeOptions::new()
            .fat_type(FatType::Fat32)
            .bytes_per_cluster(512);
        format(64, true, options)
    }

    fn mount(disk: &RamDisk) -> Volume<RamDisk> {
        block_on(Volume::mount(disk.clone())).unwrap()
    }

    /// Byte `i` of the test pattern: word `i / 4` little-endian, word `k` = `k * 0x9E37_79B9`.
    fn pattern(i: u32) -> u8 {
        (i / 4).wrapping_mul(0x9E37_79B9).to_le_bytes()[(i % 4) as usize]
    }

    fn patterned(len: u32) -> Vec<u8> {
        (0..len).map(pattern).collect()
    }

    const MIB: u32 = 1 << 20;

    #[test]
    fn mounts_an_mbr_partition() {
        let vol = mount(&formatted(true));
        block_on(vol.write("A.TXT", b"mbr")).unwrap();
        let mut buf = [0; 8];
        assert_eq!(block_on(vol.read("A.TXT", &mut buf)), Ok(3));
        assert_eq!(&buf[..3], b"mbr");
    }

    #[test]
    fn mounts_a_superfloppy() {
        let vol = mount(&formatted(false));
        block_on(vol.write("A.TXT", b"whole")).unwrap();
        let mut buf = [0; 8];
        assert_eq!(block_on(vol.read("A.TXT", &mut buf)), Ok(5));
        assert_eq!(&buf[..5], b"whole");
    }

    #[test]
    fn blank_card_is_not_fat() {
        assert!(matches!(
            block_on(Volume::mount(blank(1))),
            Err(FatError::NotFat)
        ));
    }

    #[test]
    fn write_then_read_round_trip_survives_a_remount() {
        let disk = formatted(true);
        block_on(mount(&disk).write("SDKTEST.TXT", b"deluge sdk")).unwrap();
        let vol = mount(&disk);
        let mut buf = [0; 32];
        assert_eq!(block_on(vol.read("SDKTEST.TXT", &mut buf)), Ok(10));
        assert_eq!(&buf[..10], b"deluge sdk");
    }

    #[test]
    fn write_truncates_a_longer_file() {
        let disk = formatted(true);
        let vol = mount(&disk);
        block_on(vol.write("T.BIN", &[7; 2000])).unwrap();
        block_on(vol.write("T.BIN", &[1; 10])).unwrap();
        drop(vol);
        let mut buf = [0; 4096];
        assert_eq!(block_on(mount(&disk).read("T.BIN", &mut buf)), Ok(10));
    }

    #[test]
    fn read_after_write_in_one_mount() {
        let vol = mount(&formatted(true));
        block_on(vol.write("R.TXT", b"first")).unwrap();
        block_on(vol.write("R.TXT", b"second")).unwrap();
        let mut buf = [0; 16];
        assert_eq!(block_on(vol.read("R.TXT", &mut buf)), Ok(6));
        assert_eq!(&buf[..6], b"second");
    }

    #[test]
    fn read_is_capped_at_the_buffer() {
        let vol = mount(&formatted(true));
        let data = patterned(2000);
        block_on(vol.write("C.BIN", &data)).unwrap();
        let mut buf = [0; 64];
        assert_eq!(block_on(vol.read("C.BIN", &mut buf)), Ok(64));
        assert_eq!(&buf[..], &data[..64]);
    }

    #[test]
    fn missing_file_is_not_found() {
        let vol = mount(&formatted(true));
        let mut buf = [0; 4];
        assert_eq!(
            block_on(vol.read("NOPE.BIN", &mut buf)),
            Err(FatError::NotFound)
        );
        assert!(matches!(
            block_on(vol.open("NOPE.BIN")),
            Err(FatError::NotFound)
        ));
    }

    #[test]
    fn long_names_open_as_written() {
        let disk = formatted(true);
        block_on(mount(&disk).write("bad_apple.vid", b"frames")).unwrap();
        let mut buf = [0; 8];
        assert_eq!(
            block_on(mount(&disk).read("bad_apple.vid", &mut buf)),
            Ok(6)
        );
        assert_eq!(&buf[..6], b"frames");
    }

    #[test]
    fn empty_file_reads_nothing() {
        let vol = mount(&formatted(true));
        block_on(vol.write("E.BIN", b"")).unwrap();
        block_on(async {
            let mut f = vol.open("E.BIN").await.unwrap();
            assert_eq!(f.len(), 0);
            let mut buf = [0; 16];
            assert_eq!(f.read(&mut buf).await, Ok(0));
            f.close().await.unwrap();
        });
        let mut buf = [0; 16];
        assert_eq!(block_on(vol.read("E.BIN", &mut buf)), Ok(0));
    }

    /// The disk with a 1 MiB patterned file, `S.BIN`, written and the volume remounted.
    fn with_stream_file() -> RamDisk {
        let disk = formatted(true);
        block_on(mount(&disk).write("S.BIN", &patterned(MIB))).unwrap();
        disk.max_blocks_per_read.set(0);
        disk
    }

    /// Read `S.BIN` to the end in `chunk`s of `buf`, checking every byte; the bytes read.
    fn stream_check(vol: &Volume<RamDisk>, buf: &mut [u8]) -> u32 {
        block_on(async {
            let mut f = vol.open("S.BIN").await.unwrap();
            let mut at = 0u32;
            loop {
                let n = f.read(buf).await.unwrap();
                if n == 0 {
                    break;
                }
                for (i, &b) in buf[..n].iter().enumerate() {
                    assert_eq!(b, pattern(at + i as u32), "byte {}", at + i as u32);
                }
                at += n as u32;
            }
            assert_eq!(f.read(buf).await, Ok(0));
            f.close().await.unwrap();
            at
        })
    }

    #[test]
    fn streamed_read_matches_and_ends_with_zero() {
        let vol = mount(&with_stream_file());
        let len = block_on(async { vol.open("S.BIN").await.unwrap().len() });
        assert_eq!(len, MIB);
        let mut buf = AlignedBuf([0; 32768]);
        assert_eq!(stream_check(&vol, &mut buf.0), MIB);
    }

    #[test]
    fn aligned_read_reaches_the_device_in_multi_block_calls() {
        let disk = with_stream_file();
        let vol = mount(&disk);
        let mut buf = AlignedBuf([0; 32768]);
        stream_check(&vol, &mut buf.0);
        assert!(
            disk.max_blocks_per_read.get() >= 8,
            "{}",
            disk.max_blocks_per_read.get()
        );
    }

    #[test]
    fn misaligned_read_is_correct() {
        let vol = mount(&with_stream_file());
        let mut buf = AlignedBuf([0; 1024]);
        assert_eq!(stream_check(&vol, &mut buf.0[1..1001]), MIB);
    }

    #[test]
    fn seek_then_read() {
        let vol = mount(&with_stream_file());
        block_on(async {
            let mut f = vol.open("S.BIN").await.unwrap();
            let at = 512 * 1000 + 4;
            f.seek(at).await.unwrap();
            let mut buf = [0; 8];
            assert_eq!(f.read(&mut buf).await, Ok(8));
            assert_eq!(buf, core::array::from_fn(|i| pattern(at + i as u32)));
            f.seek(f.len()).await.unwrap();
            assert_eq!(f.read(&mut buf).await, Ok(0));
            assert_eq!(f.seek(f.len() + 1).await, Err(FatError::SeekPastEnd));
            f.close().await.unwrap();
        });
    }

    #[test]
    fn card_error_maps_to_card() {
        let disk = with_stream_file();
        let vol = mount(&disk);
        block_on(async {
            let mut f = vol.open("S.BIN").await.unwrap();
            let mut buf = AlignedBuf([0; 4096]);
            disk.fail_reads.set(true);
            assert_eq!(f.read(&mut buf.0).await, Err(FatError::Card));
            disk.fail_reads.set(false);
            f.seek(0).await.unwrap();
            assert_eq!(f.read(&mut buf.0).await, Ok(4096));
            assert!(
                buf.0
                    .iter()
                    .enumerate()
                    .all(|(i, &b)| b == pattern(i as u32))
            );
            f.close().await.unwrap();
        });
    }

    #[test]
    fn full_write_leaves_a_consistent_file() {
        let disk = formatted(true);
        let vol = mount(&disk);
        block_on(vol.write("A.BIN", &[0xAA; 2000])).unwrap();
        let huge = vec![0x11; 15 * MIB as usize];
        assert_eq!(block_on(vol.write("A.BIN", &huge)), Err(FatError::Full));
        drop(vol);
        // Remounted: the failed write's file is empty, and its space is free again.
        let vol = mount(&disk);
        let mut buf = [0; 16];
        assert_eq!(block_on(vol.read("A.BIN", &mut buf)), Ok(0));
        let mib = patterned(MIB);
        block_on(vol.write("B.BIN", &mib)).unwrap();
        let mut back = vec![0; MIB as usize];
        assert_eq!(block_on(vol.read("B.BIN", &mut back)), Ok(MIB as usize));
        assert!(back == mib);
    }

    #[test]
    fn a_directory_name_is_not_a_file() {
        let vol = mount(&formatted(true));
        block_on(vol.fs.root_dir().create_dir("SAMPLES")).unwrap();
        assert_eq!(
            block_on(vol.write("SAMPLES", b"x")),
            Err(FatError::InvalidName)
        );
        let mut buf = [0; 4];
        assert_eq!(
            block_on(vol.read("SAMPLES", &mut buf)),
            Err(FatError::NotFound)
        );
    }

    #[test]
    fn mbr_whose_boot_code_starts_with_a_jump_still_mounts() {
        let disk = formatted(true);
        // GRUB's MBR boot code starts with a short jump, as a FAT boot sector does.
        disk.blocks.borrow_mut()[0][..2].copy_from_slice(&[0xEB, 0x63]);
        let vol = mount(&disk);
        block_on(vol.write("A.TXT", b"mbr")).unwrap();
        let mut buf = [0; 8];
        assert_eq!(block_on(vol.read("A.TXT", &mut buf)), Ok(3));
    }

    #[test]
    fn fat32_truncate_read_after_write_and_stream() {
        let disk = formatted_fat32();
        let vol = mount(&disk);
        assert_eq!(vol.fs.fat_type(), FatType::Fat32);
        block_on(vol.write("T.BIN", &[7; 2000])).unwrap();
        block_on(vol.write("T.BIN", b"short")).unwrap();
        let mut buf = [0; 4096];
        assert_eq!(block_on(vol.read("T.BIN", &mut buf)), Ok(5));
        block_on(vol.write("S.BIN", &patterned(MIB))).unwrap();
        drop(vol);
        let vol = mount(&disk);
        assert_eq!(block_on(vol.read("T.BIN", &mut buf)), Ok(5));
        // embedded-fatfs reads at most a cluster per call: 512-byte clusters keep it to single
        // blocks, so this checks the data, not the transfer size (SD cards usually
        // come formatted with 32 KiB).
        let mut chunk = AlignedBuf([0; 32768]);
        assert_eq!(stream_check(&vol, &mut chunk.0), MIB);
    }
}

//! The drive `USB UPDATE` shows the host: a read-only-looking FAT16 volume holding
//! `INFO_UF2.TXT` and `INDEX.HTM`, whose writes go to an [`Assembler`].
//!
//! Nothing is stored: every sector is generated on read, and the FAT and directory never change,
//! so after a remount the host sees the same two files (the usual UF2 drive behaviour). The
//! geometry is fixed:
//!
//! | LBA | What |
//! |-----|------|
//! | `0` | the boot sector |
//! | `1..=32`, `33..=64` | the two FATs |
//! | `65..=96` | the root directory (512 entries) |
//! | `97..` | data: 8 sectors (4 KiB) a cluster; `INFO_UF2.TXT` in cluster 2, `INDEX.HTM` in 3 |

use core::fmt::Write as _;

use super::{Assembler, Event};

/// Sectors a cluster.
const SECTORS_PER_CLUSTER: u32 = 8;
/// Sectors a FAT.
const FAT_SECTORS: u32 = 32;
const FAT1_START: u32 = 1;
const ROOT_START: u32 = FAT1_START + 2 * FAT_SECTORS;
const ROOT_ENTRIES: u32 = 512;
const ROOT_SECTORS: u32 = ROOT_ENTRIES * 32 / 512;
/// The first data sector (cluster 2).
const DATA_START: u32 = ROOT_START + ROOT_SECTORS;
/// The files' modification date: 2026-11-14, in FAT's packed form.
const DATE: u16 = ((2026 - 1980) << 9) | (11 << 5) | 14;
/// The longest a generated file can be (one cluster).
const FILE_MAX: usize = (SECTORS_PER_CLUSTER * 512) as usize;

/// What the drive says about the unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info<'a> {
    /// The volume label, 11 bytes, space-padded.
    pub label: [u8; 11],
    pub model: &'a str,
    pub board_id: &'a str,
    pub loader_version: &'a str,
    /// Where `INDEX.HTM` sends the browser.
    pub index_url: &'a str,
    /// The app slot's image, when it holds a valid one: (length, CRC).
    pub app_slot: Option<(u32, u32)>,
}

/// The fake drive.
pub struct Volume<'a, 'b> {
    info: Info<'a>,
    asm: Assembler<'b>,
}

/// A generated file's text.
type Text = heapless::String<FILE_MAX>;

impl<'a, 'b> Volume<'a, 'b> {
    /// The drive's size in 512-byte sectors: 32 MiB, room for the largest `.uf2` (about 8 MB).
    pub const BLOCKS: u32 = 65536;

    pub fn new(info: Info<'a>, asm: Assembler<'b>) -> Self {
        Self { info, asm }
    }

    /// The assembler the drive's writes go to.
    pub fn assembler(&mut self) -> &mut Assembler<'b> {
        &mut self.asm
    }

    fn info_text(&self) -> Text {
        let mut t = Text::new();
        let i = &self.info;
        // A full buffer only truncates the text; the files are far shorter than a cluster.
        let _ = write!(
            t,
            "UF2 Bootloader {}\r\nModel: {}\r\nBoard-ID: {}\r\n",
            i.loader_version, i.model, i.board_id
        );
        let _ = match i.app_slot {
            Some((len, crc)) => write!(t, "App slot: len {len}, crc 0x{crc:08X}\r\n"),
            None => write!(t, "App slot: empty\r\n"),
        };
        t
    }

    fn index_text(&self) -> Text {
        let mut t = Text::new();
        let _ = write!(
            t,
            "<!doctype html><meta http-equiv=\"refresh\" content=\"0; url={}\">\r\n",
            self.info.index_url
        );
        t
    }

    /// Generate sector `lba`.
    pub fn read(&self, lba: u32, out: &mut [u8; 512]) {
        out.fill(0);
        match lba {
            0 => self.boot_sector(out),
            l if (FAT1_START..ROOT_START).contains(&l) => {
                // The first sector of each FAT: the media and end-of-chain entries, and the two
                // files' one-cluster chains.
                if (l - FAT1_START) % FAT_SECTORS == 0 {
                    for (n, v) in [0xFFF8u16, 0xFFFF, 0xFFFF, 0xFFFF].iter().enumerate() {
                        out[n * 2..n * 2 + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            ROOT_START => self.root_sector(out),
            l if l >= DATA_START => {
                let cluster = 2 + (l - DATA_START) / SECTORS_PER_CLUSTER;
                let off = ((l - DATA_START) % SECTORS_PER_CLUSTER * 512) as usize;
                let text = match cluster {
                    2 => self.info_text(),
                    3 => self.index_text(),
                    _ => return,
                };
                let b = text.as_bytes();
                if off < b.len() {
                    let n = (b.len() - off).min(512);
                    out[..n].copy_from_slice(&b[off..off + n]);
                }
            }
            _ => {}
        }
    }

    /// Take sector `lba` the host wrote. The reserved, FAT and directory sectors are the host's
    /// own bookkeeping and never reach the assembler.
    pub fn write(&mut self, lba: u32, data: &[u8; 512]) -> Event {
        if lba < DATA_START {
            return Event::Ignored;
        }
        self.asm.write(data)
    }

    fn boot_sector(&self, out: &mut [u8; 512]) {
        out[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
        out[3..11].copy_from_slice(b"DLG UF2 ");
        out[11..13].copy_from_slice(&512u16.to_le_bytes());
        out[13] = SECTORS_PER_CLUSTER as u8;
        out[14..16].copy_from_slice(&(FAT1_START as u16).to_le_bytes());
        out[16] = 2;
        out[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
        // Total sectors: 65536 doesn't fit the 16-bit field, so the 32-bit one carries it.
        out[21] = 0xF8;
        out[22..24].copy_from_slice(&(FAT_SECTORS as u16).to_le_bytes());
        out[24..26].copy_from_slice(&1u16.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[32..36].copy_from_slice(&Self::BLOCKS.to_le_bytes());
        out[36] = 0x80;
        out[38] = 0x29;
        out[39..43].copy_from_slice(&0x5546_3200u32.to_le_bytes());
        out[43..54].copy_from_slice(&self.info.label);
        out[54..62].copy_from_slice(b"FAT16   ");
        out[510] = 0x55;
        out[511] = 0xAA;
    }

    fn root_sector(&self, out: &mut [u8; 512]) {
        let entry =
            |out: &mut [u8; 512], n: usize, name: &[u8; 11], attr: u8, cluster: u16, size: u32| {
                let e = &mut out[n * 32..n * 32 + 32];
                e[..11].copy_from_slice(name);
                e[11] = attr;
                e[16..18].copy_from_slice(&DATE.to_le_bytes());
                e[18..20].copy_from_slice(&DATE.to_le_bytes());
                e[24..26].copy_from_slice(&DATE.to_le_bytes());
                e[26..28].copy_from_slice(&cluster.to_le_bytes());
                e[28..32].copy_from_slice(&size.to_le_bytes());
            };
        // Volume label, then the two read-only files.
        entry(out, 0, &self.info.label, 0x08, 0, 0);
        entry(
            out,
            1,
            b"INFO_UF2TXT",
            0x01,
            2,
            self.info_text().len() as u32,
        );
        entry(
            out,
            2,
            b"INDEX   HTM",
            0x01,
            3,
            self.index_text().len() as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uf2::{Limits, Target};
    use aligned::{A4, Aligned};
    use block_device_adapters::{BufStream, StreamSlice};
    use block_device_driver::BlockDevice;
    use deluge_image::app::encode_app_header;
    use deluge_image::slot::{self, VARIANT_OLED};
    use deluge_image::uf2::{self, DELUGE_OLED};
    use embassy_futures::block_on;
    use embedded_fatfs::{FileSystem, FsOptions};
    use embedded_io_async::{Read, Write};
    use std::boxed::Box;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::string::String;
    use std::vec;
    use std::vec::Vec;

    type Vol = Volume<'static, 'static>;

    const INFO: Info<'static> = Info {
        label: *b"DELUGE     ",
        model: "Deluge OLED",
        board_id: "DELUGE-OLED",
        loader_version: "0.2.0",
        index_url: "https://example.invalid/app-loader.md",
        app_slot: None,
    };

    fn volume() -> Vol {
        let buf: &'static mut [u8] = Box::leak(vec![0u8; 0x2C_0000].into_boxed_slice());
        let limits = Limits {
            families: DELUGE_OLED,
            max_app: 0x2C_0000,
            max_loader: slot::SLOT_LEN as usize,
        };
        Volume::new(INFO, Assembler::new(buf, limits))
    }

    /// The host's view of the drive: what it wrote reads back (as a real disk would, and as the
    /// host's own cache makes it look), and every write also reaches the [`Volume`].
    #[derive(Clone)]
    struct HostDisk {
        vol: Rc<RefCell<Vol>>,
        written: Rc<RefCell<HashMap<u32, [u8; 512]>>>,
        events: Rc<RefCell<Vec<Event>>>,
    }

    impl HostDisk {
        fn new(vol: Vol) -> Self {
            Self {
                vol: Rc::new(RefCell::new(vol)),
                written: Rc::default(),
                events: Rc::default(),
            }
        }
    }

    impl BlockDevice<512> for HostDisk {
        type Error = ();
        type Align = A4;

        async fn read(&mut self, at: u32, data: &mut [Aligned<A4, [u8; 512]>]) -> Result<(), ()> {
            for (i, b) in data.iter_mut().enumerate() {
                let lba = at + i as u32;
                match self.written.borrow().get(&lba) {
                    Some(w) => b.copy_from_slice(w),
                    None => self.vol.borrow().read(lba, b),
                }
            }
            Ok(())
        }

        async fn write(&mut self, at: u32, data: &[Aligned<A4, [u8; 512]>]) -> Result<(), ()> {
            for (i, b) in data.iter().enumerate() {
                let lba = at + i as u32;
                self.written.borrow_mut().insert(lba, **b);
                let e = self.vol.borrow_mut().write(lba, b);
                self.events.borrow_mut().push(e);
            }
            Ok(())
        }

        async fn size(&mut self) -> Result<u64, ()> {
            Ok(Vol::BLOCKS as u64 * 512)
        }
    }

    async fn mount(
        disk: HostDisk,
    ) -> FileSystem<
        StreamSlice<BufStream<HostDisk, 512>>,
        embedded_fatfs::DefaultTimeProvider,
        embedded_fatfs::LossyOemCpConverter,
    > {
        let slice = StreamSlice::new(BufStream::new(disk), 0, Vol::BLOCKS as u64 * 512)
            .await
            .unwrap();
        FileSystem::new(slice, FsOptions::new()).await.unwrap()
    }

    fn app_uf2() -> (Vec<u8>, Vec<u8>) {
        let body: Vec<u8> = (0..20_000).map(|i| (i * 11 + 9) as u8).collect();
        let mut image = encode_app_header(&body, VARIANT_OLED, 0x2002_0000, 0x2002_0000).to_vec();
        image.extend_from_slice(&body);
        let file: Vec<u8> = uf2::encode(&image, DELUGE_OLED.app).flatten().collect();
        (image, file)
    }

    #[test]
    fn mounts_and_lists_two_files() {
        let disk = HostDisk::new(volume());
        block_on(async {
            let fs = mount(disk).await;
            let root = fs.root_dir();
            let mut names = Vec::new();
            let mut it = root.iter();
            while let Some(e) = it.next().await {
                names.push(
                    String::from_utf8_lossy(e.unwrap().short_file_name_as_bytes()).into_owned(),
                );
            }
            names.sort();
            assert_eq!(names, ["INDEX.HTM", "INFO_UF2.TXT"]);
            let mut f = root.open_file("INFO_UF2.TXT").await.unwrap();
            let mut buf = [0u8; 256];
            let n = f.read(&mut buf).await.unwrap();
            let text = core::str::from_utf8(&buf[..n]).unwrap();
            assert!(text.starts_with("UF2 Bootloader 0.2.0\r\n"), "{text}");
            assert!(text.contains("Model: Deluge OLED\r\n"));
            assert!(text.contains("Board-ID: DELUGE-OLED\r\n"));
            assert!(text.contains("App slot: empty\r\n"));
            let mut f = root.open_file("INDEX.HTM").await.unwrap();
            let n = f.read(&mut buf).await.unwrap();
            let html = core::str::from_utf8(&buf[..n]).unwrap();
            assert!(
                html.contains("url=https://example.invalid/app-loader.md"),
                "{html}"
            );
        });
    }

    #[test]
    fn info_shows_the_app_slot() {
        let mut v = volume();
        v.info.app_slot = Some((1234, 0xDEAD_BEEF));
        let disk = HostDisk::new(v);
        block_on(async {
            let fs = mount(disk).await;
            let mut f = fs.root_dir().open_file("INFO_UF2.TXT").await.unwrap();
            let mut buf = [0u8; 256];
            let n = f.read(&mut buf).await.unwrap();
            let text = core::str::from_utf8(&buf[..n]).unwrap();
            assert!(
                text.contains("App slot: len 1234, crc 0xDEADBEEF\r\n"),
                "{text}"
            );
        });
    }

    #[test]
    fn volume_label_is_info_s() {
        let disk = HostDisk::new(volume());
        block_on(async {
            let fs = mount(disk).await;
            assert_eq!(fs.volume_label_as_bytes(), b"DELUGE");
        });
    }

    #[test]
    fn os_writes_through_fatfs_complete_the_file() {
        let disk = HostDisk::new(volume());
        let (image, file) = app_uf2();
        block_on(async {
            let fs = mount(disk.clone()).await;
            let root = fs.root_dir();
            let mut junk = root.create_file("._FIRMWARE.UF2").await.unwrap();
            junk.write_all(&[0x5A; 4096]).await.unwrap();
            junk.flush().await.unwrap();
            root.create_dir(".fseventsd").await.unwrap();
            let mut f = root.create_file("FIRMWARE.UF2").await.unwrap();
            f.write_all(&file).await.unwrap();
            f.flush().await.unwrap();
            root.create_dir(".Trashes").await.unwrap();
            fs.flush().await.unwrap();
        });
        let events = disk.events.borrow();
        assert!(
            events.contains(&Event::Complete {
                target: Target::App,
                len: image.len()
            }),
            "no Complete in {} events",
            events.len()
        );
        assert_eq!(disk.vol.borrow_mut().assembler().image(), &image[..]);
    }

    #[test]
    fn metadata_writes_are_ignored() {
        let mut v = volume();
        let (_, file) = app_uf2();
        let block: [u8; 512] = file[..512].try_into().unwrap();
        // A UF2 block written into the FAT region is the host's, not the file's.
        assert_eq!(v.write(1, &block), Event::Ignored);
        assert_eq!(v.write(96, &block), Event::Ignored);
        assert_ne!(v.write(DATA_START, &block), Event::Ignored);
    }

    #[test]
    fn the_geometry_is_fat16() {
        // FAT16 needs at least 4085 clusters, and the FATs must cover every one.
        let clusters = (Vol::BLOCKS - DATA_START) / SECTORS_PER_CLUSTER;
        assert!((4085..65525).contains(&clusters));
        assert!(FAT_SECTORS * 512 / 2 >= clusters + 2);
    }
}

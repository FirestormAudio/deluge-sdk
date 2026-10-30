//! Deluge USB Mass Storage firmware image.
//!
//! Turns the Deluge into a USB SD-card reader/writer.  When connected to a host
//! the device enumerates as a USB Mass Storage (Bulk-Only Transport, SCSI
//! transparent) device exposing the inserted SD card as a raw block device, so
//! the host OS mounts the card's FAT volume directly.
//!
//! ## Runtime role
//! - Initialises the platform, heaps, PIC/OLED transport, and the task executor.
//! - Brings USB0 up in **device mode** with a single MSC interface.
//! - Runs the Bulk-Only Transport / SCSI command loop, bridging USB bulk
//!   endpoints to [`deluge_bsp::sd`] block read/write.
//! - Renders a live throughput display (TX/RX MB/s and cumulative MB) to the OLED.
//!
//! ## Concurrency note
//! While acting as USB mass storage the **host owns the filesystem**.  This app
//! performs raw block passthrough only and never mounts or writes FAT itself,
//! so the host's cached view of the filesystem is never invalidated behind its
//! back.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod tasks;

use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use log::{debug, error, info};

use embassy_executor::{Executor, Spawner};

use deluge_bsp::cv_gate;
use deluge_bsp::uart as bsp_uart;
use rza1_alloc as allocator;
use rza1_hal::usb::init_device_mode;

rza1_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1_hal::usb::DeviceInterruptHandler<rza1_hal::peripherals::USB0>;
});

unsafe extern "C" {
    /// Start of the free SRAM heap region (set by the linker script).
    static __sram_heap_start: u8;
    /// End of the free SRAM heap region (start of RTT/stack reservation).
    static __sram_heap_end: u8;
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// USB descriptor static buffers
// ---------------------------------------------------------------------------
//
// Must live here (in the root crate) so the `'static` lifetimes required by
// `embassy_usb::Builder` and `builder.handler()` are satisfied.

static mut USB_CONFIG_DESC: [u8; 256] = [0; 256];
static mut USB_BOS_DESC: [u8; 64] = [0; 64];
static mut USB_MSOS_DESC: [u8; 0] = [];
static mut USB_CONTROL_BUF: [u8; 64] = [0; 64];
/// Backing storage for the MSC class handler (must be `'static`).
static mut MSC_CLASS_BUF: MaybeUninit<usb_classes::msc::MscClass> = MaybeUninit::uninit();

static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();

#[deluge_bsp::entry]
fn main() -> ! {
    // With the `rtt` feature, log over a 16 KB RTT up-channel in `.rtt_buffer`
    // (uncached RAM).
    #[cfg(feature = "rtt")]
    {
        let channels = rtt_target::rtt_init! {
            up: {
                0: {
                    size: 16384,
                    name: "Terminal",
                    section: ".rtt_buffer"
                }
            }
            section_cb: ".rtt_buffer"
        };
        rtt_target::set_print_channel(channels.up.0);
        rtt_target::init_logger_with_level(log::LevelFilter::Debug);
    }
    info!("Deluge USB Mass Storage firmware starting");

    // Initialise the SRAM heap before any allocation from internal RAM.
    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }
    info!("SRAM heap: initialised ({} KB)", {
        let s = core::ptr::addr_of!(__sram_heap_end) as usize
            - core::ptr::addr_of!(__sram_heap_start) as usize;
        s / 1024
    });

    let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
    info!("system: module clocks, MMU, cache, SDRAM, GIC, OSTM, time driver ready");

    // Initialise the SDRAM heap now that the SDRAM window is accessible.
    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };
    info!("SDRAM heap: initialised (64 MB)");

    info!("GPIO: configuring heartbeat LED...");
    let sync_led = rza1_hal::gpio::Output::new(board.pins.sync_led, rza1_hal::gpio::Level::Low);

    info!("UART: initialising PIC...");
    bsp_uart::init_pic(
        board.pic_uart,
        board.pins.pic,
        board.dma.pic_rx,
        board.dma.pic_tx,
        31_250,
    );
    // The SD slot's controller, once; `sd::init` identifies the card when it is needed.
    deluge_bsp::sd::init_hw(
        board.sd_card,
        board.pins.sd,
        board.dma.sd_tx,
        board.dma.sd_rx,
    );
    info!("UART: SCIF1 @ 31 250 baud");

    // RSPI0 is shared between OLED (8-bit) and CV DAC (32-bit). The bus brings it up;
    // cv_gate::init() runs the DAC's linearity sequence; oled::init() then switches
    // it to 8-bit mode for the panel.
    deluge_bsp::bus::init(board.shared_spi, board.pins.spi, board.dma.oled);
    cv_gate::init(board.pins.cv_gate);
    info!("RSPI0: initialised via cv_gate::init");

    // ── USB0 device mode ──────────────────────────────────────────────────
    let (usb_device, ep_out, ep_in) = unsafe {
        let (_port, driver) = init_device_mode(board.usb0, UsbIrqs);
        // Example/development USB identity — self-contained, NOT a product ID.
        // See `deluge_bsp::usb::ids` for the product-identity rules and why
        // distinct PIDs matter on macOS. TODO: use IDs you own (e.g. a pid.codes
        // prototype ID) before distributing this firmware.
        const USB_VID: u16 = 0x16D0;
        const USB_PID: u16 = 0x0EDA;
        let mut config = embassy_usb::Config::new(USB_VID, USB_PID);
        config.manufacturer = Some("Synthstrom Audible");
        config.product = Some("Deluge SD Card");
        config.self_powered = false;
        config.max_power = 250; // 500 mA

        let mut builder = embassy_usb::Builder::new(
            driver,
            config,
            &mut *core::ptr::addr_of_mut!(USB_CONFIG_DESC),
            &mut *core::ptr::addr_of_mut!(USB_BOS_DESC),
            &mut *core::ptr::addr_of_mut!(USB_MSOS_DESC),
            &mut *core::ptr::addr_of_mut!(USB_CONTROL_BUF),
        );

        // Single MSC interface with one bulk IN + bulk OUT endpoint pair.
        // 512-byte bulk endpoints: the RUSB1 PHY always negotiates high speed,
        // and USB 2.0 requires HS bulk endpoints to use wMaxPacketSize 512.
        let (msc, ep_out, ep_in) = usb_classes::msc::MscClass::new(&mut builder, 512);
        let msc_ref = (&mut *core::ptr::addr_of_mut!(MSC_CLASS_BUF)).write(msc);
        builder.handler(msc_ref);

        (builder.build(), ep_out, ep_in)
    };
    info!("USB: UsbDevice built (MSC)");

    debug!("enabling IRQ...");
    unsafe { cortex_ar::interrupt::enable() };
    info!("IRQ: enabled — starting Embassy tasks");

    #[allow(static_mut_refs)]
    let executor = unsafe {
        EXECUTOR.write(Executor::new());
        EXECUTOR.assume_init_mut()
    };
    executor.run(|spawner: Spawner| {
        spawner.spawn(tasks::blink::blink_task(sync_led).unwrap());
        spawner.spawn(tasks::usb::usb_task(usb_device).unwrap());
        spawner.spawn(tasks::msc::msc_task(ep_in, ep_out).unwrap());
        spawner.spawn(tasks::pic::pic_task().unwrap());
        spawner.spawn(tasks::oled::oled_task().unwrap());
    });
}

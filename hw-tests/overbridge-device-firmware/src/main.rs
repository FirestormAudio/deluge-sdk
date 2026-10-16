//! Deluge Overbridge **device** validation firmware (spec §8.4 step 6).
//!
//! Presents the Overbridge 2 device class with a Digitakt's channel counts (12 to the host, 2
//! from it) and USB-MIDI, sends a DC level per stem, and logs the stream and the host's name
//! once a second. By default it uses a development identity, which neither the Deluge host
//! class nor Overwitch binds; build with `--features elektron-vid` on your own bench to test
//! against them.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod tasks;

use core::mem::MaybeUninit;
use core::panic::PanicInfo;

use embassy_executor::{Executor, Spawner};
use log::{error, info};

use deluge_bsp::uart as bsp_uart;
use rza1_alloc as allocator;
use rza1l_hal::usb::init_device_mode;
use usb_classes::overbridge::device::{self, OverbridgeConfig, State};
use usb_classes::overbridge::proto::Format;
use usb_classes::overbridge::stems::OverbridgeInfo;

use tasks::overbridge::{CH_FROM_HOST, CH_TO_HOST, STEMS};

rza1l_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1l_hal::usb::DeviceInterruptHandler<rza1l_hal::peripherals::USB0>;
});

/// pid.codes' test VID/PID: for development only, never distributed.
#[cfg(not(feature = "elektron-vid"))]
const USB_ID: (u16, u16) = (0x1209, 0x0001);
/// Elektron's VID with a PID no device table lists (spec §7.2; bench only).
#[cfg(feature = "elektron-vid")]
const USB_ID: (u16, u16) = (0x1935, 0x0BF0);

unsafe extern "C" {
    static __sram_heap_start: u8;
    static __sram_heap_end: u8;
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();
static mut USB_CONFIG_DESC: [u8; 512] = [0; 512];
static mut USB_BOS_DESC: [u8; 64] = [0; 64];
static mut USB_MSOS_DESC: [u8; 0] = [];
static mut USB_CONTROL_BUF: [u8; 64] = [0; 64];
static mut OVERBRIDGE_STATE: State<'static> = State::new();

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    #[cfg(feature = "rtt")]
    {
        let channels = rtt_target::rtt_init! {
            up: { 0: { size: 16384, name: "Terminal", section: ".rtt_buffer" } }
            section_cb: ".rtt_buffer"
        };
        rtt_target::set_print_channel(channels.up.0);
        rtt_target::init_logger_with_level(log::LevelFilter::Debug);
    }
    info!("Overbridge device validation firmware starting");

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }
    let board = deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG));
    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };

    unsafe { rza1l_hal::gpio::set_as_output(6, 7) };
    bsp_uart::init_pic(board.pic_uart, board.dma.pic_rx, board.dma.pic_tx, 31_250);

    let (_port, driver) = init_device_mode(board.usb0, UsbIrqs);
    let mut config = embassy_usb::Config::new(USB_ID.0, USB_ID.1);
    config.manufacturer = Some("Deluge SDK");
    config.product = Some("Deluge Overbridge test");
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;
    config.self_powered = true;
    config.max_power = 0;

    // SAFETY: once, before the executor starts; nothing else touches these statics.
    #[allow(static_mut_refs)]
    let (usb, sender, receiver, shared) = unsafe {
        let mut builder = embassy_usb::Builder::new(
            driver,
            config,
            &mut *core::ptr::addr_of_mut!(USB_CONFIG_DESC),
            &mut *core::ptr::addr_of_mut!(USB_BOS_DESC),
            &mut *core::ptr::addr_of_mut!(USB_MSOS_DESC),
            &mut *core::ptr::addr_of_mut!(USB_CONTROL_BUF),
        );
        let (sender, receiver, shared) = device::new(
            &mut builder,
            &mut *core::ptr::addr_of_mut!(OVERBRIDGE_STATE),
            OverbridgeConfig {
                format: Format::V2,
                ch_to_host: CH_TO_HOST as u8,
                ch_from_host: CH_FROM_HOST as u8,
                name: "Deluge",
            },
        );
        // USB-MIDI on interfaces 4-5, as on the Digitakt. Its endpoints aren't serviced here.
        let _midi = embassy_usb::class::midi::MidiClass::new(&mut builder, 1, 1, 512);
        (builder.build(), sender, receiver, shared)
    };
    let info = OverbridgeInfo {
        pid: USB_ID.1,
        name: "Deluge",
        ch_from: CH_TO_HOST as u8,
        ch_to: CH_FROM_HOST as u8,
        format: Format::V2,
    };
    STEMS
        .begin(info, CH_FROM_HOST, CH_TO_HOST)
        .expect("the stem rings hold the channels");

    unsafe { cortex_ar::interrupt::enable() };
    info!("IRQ enabled — starting tasks");

    #[allow(static_mut_refs)]
    let executor = unsafe {
        EXECUTOR.write(Executor::new());
        EXECUTOR.assume_init_mut()
    };
    executor.run(|spawner: Spawner| {
        spawner.spawn(tasks::blink::blink_task().unwrap());
        spawner.spawn(tasks::pic::pic_task().unwrap());
        spawner.spawn(tasks::overbridge::usb_task(usb).unwrap());
        spawner.spawn(tasks::overbridge::sender_task(sender).unwrap());
        spawner.spawn(tasks::overbridge::receiver_task(receiver).unwrap());
        spawner.spawn(tasks::overbridge::stems_task().unwrap());
        spawner.spawn(tasks::overbridge::report_task(shared).unwrap());
    })
}

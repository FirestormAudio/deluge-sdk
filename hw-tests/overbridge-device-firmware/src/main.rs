//! Deluge Overbridge **device** validation firmware (spec §8.4 step 6, and §5.6's device-side
//! switching).
//!
//! Presents the Overbridge 2 device class with a Digitakt's channel counts (12 to the host, 2
//! from it) and USB-MIDI, sends a DC level per stem, and logs the stream once a second. By
//! default it uses a development identity, which neither the Deluge host class nor Overwitch
//! binds; build with `--features elektron-id` on your own bench to test against them. With
//! `face-switch` it alternates every 30 s between the Overbridge face and a UAC2 face
//! ([`tasks::faces`]).

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
use usb_classes::overbridge::proto::Format;
use usb_classes::overbridge::stems::OverbridgeInfo;

use tasks::overbridge::{CH_FROM_HOST, CH_TO_HOST, STEMS};

rza1_hal::bind_interrupts!(pub(crate) struct UsbIrqs {
    USB0 => rza1_hal::usb::DeviceInterruptHandler<rza1_hal::peripherals::USB0>;
});

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
    let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };

    unsafe { rza1_hal::gpio::set_as_output(6, 7) };
    bsp_uart::init_pic(board.pic_uart, board.dma.pic_rx, board.dma.pic_tx, 31_250);

    let info = OverbridgeInfo {
        pid: 0,
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
        spawner.spawn(tasks::faces::faces_task(board.usb0).unwrap());
        spawner.spawn(tasks::overbridge::stems_task().unwrap());
        spawner.spawn(tasks::overbridge::report_task().unwrap());
    })
}

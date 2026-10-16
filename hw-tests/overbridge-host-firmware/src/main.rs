//! Deluge Overbridge host **validation** firmware (spec §8.4 steps 2–4).
//!
//! Brings USB0 up in host mode and runs `usb_host_supervisor`, which binds an Elektron
//! device in Overbridge mode to `usb_classes::host::overbridge`. Once a second it logs the
//! device, the stream counters and every stem's peak; it sends a 1 kHz tone to the device.

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
use rza1l_hal::usb::init_host_mode;

rza1l_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1l_hal::usb::HostInterruptHandler<rza1l_hal::peripherals::USB0>;
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
    info!("Overbridge host validation firmware starting");

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }
    let board = deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG));
    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };

    unsafe { rza1l_hal::gpio::set_as_output(6, 7) };
    bsp_uart::init_pic(board.pic_uart, board.dma.pic_rx, board.dma.pic_tx, 31_250);

    let (_port, host_driver) = init_host_mode(board.usb0, UsbIrqs);
    info!("USB: host driver ready (USB0 host mode)");

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
        spawner.spawn(tasks::report::report_task().unwrap());
        spawner.spawn(tasks::tone::tone_task().unwrap());
        // The supervisor enumerates and binds the Overbridge driver, which spawns the stream
        // task. It needs the spawner for that.
        spawner.spawn(
            deluge_bsp::usb::host::usb_host_supervisor(host_driver, spawner)
                .expect("supervisor task pool full"),
        );
    })
}

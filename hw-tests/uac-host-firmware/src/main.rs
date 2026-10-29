//! Deluge UAC host **validation** firmware.
//!
//! Brings USB0 up in host mode, runs `usb_host_supervisor` (which auto-binds the
//! `usb_classes::host::uac` driver on connect), and plays captured audio on the
//! Deluge's codec through the SCUX (its 2SRC converting the device's 48 kHz to
//! the codec's 44.1 kHz), exercising the host UAC capture stack on hardware.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod tasks;

use core::mem::MaybeUninit;
use core::panic::PanicInfo;

use embassy_executor::{Executor, Spawner};
use log::{error, info};

use deluge_bsp::{cv_gate, uart as bsp_uart};
use rza1_alloc as allocator;
use rza1_hal::usb::init_host_mode;

rza1_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1_hal::usb::HostInterruptHandler<rza1_hal::peripherals::USB0>;
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
    info!("UAC host validation firmware starting");

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }
    let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };

    unsafe { rza1_hal::gpio::set_as_output(6, 7) };
    bsp_uart::init_pic(board.pic_uart, board.pins.pic, board.dma.pic_rx, board.dma.pic_tx, 31_250);
    deluge_bsp::bus::init(board.shared_spi, board.pins.spi, board.dma.oled);
    cv_gate::init(board.pins.cv_gate);

    // The codec through the SCUX, its 2SRC converting the device's 48 kHz to 44.1 kHz; the UAC
    // host streams at 48 kHz so its resampler only trims clock drift.
    deluge_bsp::audio::init_with_scux_src(
        board.audio_ssi,
        board.pins.audio,
        board.dma.ssi_rx,
        board.dma.scux_ffd0,
        48_000,
    );
    deluge_bsp::usb::host::set_uac_sample_rate(48_000);

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
        spawner.spawn(deluge_bsp::cpu_load::spin_task().unwrap());
        spawner.spawn(deluge_bsp::cpu_load::report_task().unwrap());
        spawner.spawn(tasks::blink::blink_task().unwrap());
        spawner.spawn(tasks::pic::pic_task().unwrap());
        spawner.spawn(tasks::oled::oled_task().unwrap());
        spawner.spawn(tasks::speaker::speaker_task().unwrap());
        // The supervisor enumerates + binds the UAC driver, which spawns the
        // capture/playback pump. It needs the spawner to spawn that child task.
        spawner.spawn(
            deluge_bsp::usb::host::usb_host_supervisor(host_driver, spawner)
                .expect("supervisor task pool full"),
        );
    })
}

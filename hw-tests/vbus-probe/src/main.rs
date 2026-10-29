//! USB VBUS-sense probe firmware.
//!
//! A diagnostic image: it brings up the platform + OLED, puts USB0
//! into **host mode** (the same state the UAC host firmware runs in), then
//! continuously reads the RUSB1 VBUS monitor (`INTSTS0.VBSTS`) and reports
//! PRESENT / ABSENT on the OLED and over RTT.
//!
//! Purpose: the RZ/A1L RUSB1 has no VBUS-drive bit (`DVSTCTR0` bits 11:9 are
//! reserved) — host-mode VBUS must be switched on externally. Use this to check
//! whether the USB port is actually powered while in host mode: plug a device
//! and watch whether VBUS reads PRESENT. If it stays ABSENT, the connected
//! device is unpowered (which stalls enumeration).
//!
//! Build: `cargo build-fw -p vbus-probe --features rtt`.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod tasks;

use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use log::{error, info};

use embassy_executor::{Executor, Spawner};

use rza1_alloc as allocator;
use deluge_bsp::cv_gate;
use deluge_bsp::uart as bsp_uart;

rza1_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1_hal::usb::HostInterruptHandler<rza1_hal::peripherals::USB0>;
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

static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
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
    info!("Deluge VBUS-probe firmware starting");

    // Initialise the SRAM heap before any allocation from internal RAM.
    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }

    let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
    info!("system: clocks, MMU, cache, SDRAM, GIC, OSTM, time driver ready");

    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };

    // Heartbeat LED pin (unused beyond init; keeps parity with other firmwares).
    unsafe { rza1_hal::gpio::set_as_output(6, 7) };

    info!("UART: initialising PIC...");
    bsp_uart::init_pic(board.pic_uart, board.pins.pic, board.dma.pic_rx, board.dma.pic_tx, 31_250);

    // RSPI0 is shared between OLED (8-bit) and CV DAC (32-bit); the bus brings it up.
    deluge_bsp::bus::init(board.shared_spi, board.pins.spi, board.dma.oled);
    unsafe { cv_gate::init() };
    info!("RSPI0: initialised via cv_gate::init");

    // Bring USB0 up in host mode so VBSTS reflects the real host-mode VBUS
    // state. The returned driver is unused — this firmware only reads the VBUS
    // monitor; the host handler services whatever interrupts the port raises.
    let (_port, _driver) = rza1_hal::usb::init_host_mode(board.usb0, UsbIrqs);
    info!("USB: USB0 in host mode (VBUS sense only)");

    unsafe { cortex_ar::interrupt::enable() };
    info!("IRQ: enabled — starting Embassy tasks");

    #[allow(static_mut_refs)]
    let executor = unsafe {
        EXECUTOR.write(Executor::new());
        EXECUTOR.assume_init_mut()
    };
    executor.run(|spawner: Spawner| {
        spawner.spawn(tasks::pic::pic_task().unwrap());
        spawner.spawn(tasks::probe::probe_task().unwrap());
    });
}

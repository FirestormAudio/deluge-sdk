//! UART and RSPI loopback probe firmware.
//!
//! Runs the HAL's interrupt-driven paths on silicon with no wiring, each peripheral in its own
//! internal loopback mode:
//! - SCIF0 (the DIN MIDI port) built by `Uart::new`, receiving on RXI into a lent buffer, with
//!   SCFCR.LOOP set (TxD looped to RxD inside the SCIF);
//! - RSPI0 (the OLED/CV bus) built by the async `Rspi::new_txonly`, waiting on SPTI/SPRI, with
//!   SPPCR.SPLP set (MOSI cut off, the shift register's output looped to its input, inverted:
//!   each frame comes back as its complement).
//!
//! Every result goes to the RTT log: `loopback: PASS` or `loopback: FAIL …`.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use core::mem::MaybeUninit;
use core::panic::PanicInfo;
use log::{error, info};

use embassy_executor::{Executor, Spawner};
use embassy_time::{Duration, with_timeout};
use rza1_alloc as allocator;
use rza1_hal::mode::Async;
use rza1_hal::peripherals::{RSPI0, SCIF0};
use rza1_hal::rspi::{self, Rspi};
use rza1_hal::uart::{self, Uart, UartRx, UartTx};

unsafe extern "C" {
    /// Start of the free SRAM heap region (set by the linker script).
    static __sram_heap_start: u8;
    /// End of the free SRAM heap region (start of RTT/stack reservation).
    static __sram_heap_end: u8;
}

rza1_hal::bind_interrupts!(struct Irqs {
    BRI0 => uart::BreakInterruptHandler<SCIF0>;
    ERI0 => uart::ErrorInterruptHandler<SCIF0>;
    RXI0 => uart::RxInterruptHandler<SCIF0>;
    TXI0 => uart::TxInterruptHandler<SCIF0>;
    SPEI0 => rspi::ErrorInterruptHandler<RSPI0>;
    SPRI0 => rspi::RxInterruptHandler<RSPI0>;
    SPTI0 => rspi::TxInterruptHandler<RSPI0>;
});

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();
/// SCIF0's lent receive buffer.
static mut RX_BUF: [u8; 64] = [0; 64];

#[deluge_bsp::entry]
fn main() -> ! {
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
        rtt_target::init_logger_with_level(log::LevelFilter::Info);
    }
    info!("loopback-probe starting");

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }

    let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));

    let mut config = uart::Config::default();
    config.baudrate = 115_200;
    #[allow(static_mut_refs)]
    let rx_buf = unsafe { &mut *core::ptr::addr_of_mut!(RX_BUF) };
    let (rx, tx) =
        Uart::new(board.midi_uart, board.pins.midi.tx, board.pins.midi.rx, Irqs, rx_buf, None, config)
            .split();
    // SCFCR.LOOP: TxD to RxD inside the SCIF.
    let fcr = rza1_hal::pac::SCIF0.fcr();
    fcr.write_value(rza1_hal::pac::scif::regs::Fcr(fcr.read().0 | 1));

    let mut spi_config = rspi::Config::default();
    spi_config.frequency = 4_000_000;
    let spi = Rspi::new_txonly(board.shared_spi, board.pins.spi.sck, board.pins.spi.mosi, Irqs, spi_config);
    // SPPCR.SPLP: the shift register's output looped, inverted, to its input; MOSI cut off.
    rza1_hal::pac::RSPI0.sppcr().write_value(1);

    unsafe { cortex_ar::interrupt::enable() };

    #[allow(static_mut_refs)]
    let executor = unsafe {
        EXECUTOR.write(Executor::new());
        EXECUTOR.assume_init_mut()
    };
    executor.run(|spawner: Spawner| {
        spawner.spawn(probe(rx, tx, spi).unwrap());
    });
}

#[embassy_executor::task]
async fn probe(mut rx: UartRx<'static, Async>, tx: UartTx<'static, Async>, mut spi: Rspi<'static, Async>) {
    let mut failures = 0u32;

    // ---- UART: RXI into the lent buffer ----
    let msg = b"rza1-hal loopback";
    tx.write(msg).await;
    let mut got = [0u8; 17];
    for (i, slot) in got.iter_mut().enumerate() {
        match with_timeout(Duration::from_millis(50), rx.read_byte()).await {
            Ok(Ok(byte)) => *slot = byte,
            Ok(Err(e)) => {
                error!("loopback: FAIL uart byte {}: receive error {:?}", i, e);
                failures += 1;
                break;
            }
            Err(_) => {
                error!("loopback: FAIL uart byte {}: no byte within 50 ms (RXI never woke the reader?)", i);
                failures += 1;
                break;
            }
        }
    }
    if &got == msg {
        info!("loopback: uart OK ({} bytes through RXI)", msg.len());
    } else if failures == 0 {
        error!("loopback: FAIL uart got {:02x?}", got);
        failures += 1;
    }

    // ---- RSPI: async full duplex, 8- and 32-bit frames ----
    use embedded_hal_async::spi::SpiBus;
    let out8 = [0xA5u8, 0x3C, 0x00, 0xFF, 0x81];
    let mut in8 = [0u8; 5];
    match with_timeout(Duration::from_millis(50), SpiBus::<u8>::transfer(&mut spi, &mut in8, &out8)).await {
        Ok(Ok(())) if in8 == out8.map(|w| !w) => info!("loopback: rspi u8 OK (complements back)"),
        Ok(Ok(())) => {
            error!("loopback: FAIL rspi u8 got {:02x?}, sent {:02x?}", in8, out8);
            failures += 1;
        }
        Ok(Err(e)) => {
            error!("loopback: FAIL rspi u8: {:?}", e);
            failures += 1;
        }
        Err(_) => {
            error!("loopback: FAIL rspi u8 timed out (SPTI/SPRI never woke it?)");
            failures += 1;
        }
    }
    let out32 = [0x1234_5678u32, 0xDEAD_BEEF];
    let mut in32 = [0u32; 2];
    match with_timeout(Duration::from_millis(50), SpiBus::<u32>::transfer(&mut spi, &mut in32, &out32)).await {
        Ok(Ok(())) if in32 == out32.map(|w| !w) => info!("loopback: rspi u32 OK (complements back)"),
        Ok(Ok(())) => {
            error!("loopback: FAIL rspi u32 got {:08x?}, sent {:08x?}", in32, out32);
            failures += 1;
        }
        Ok(Err(e)) => {
            error!("loopback: FAIL rspi u32: {:?}", e);
            failures += 1;
        }
        Err(_) => {
            error!("loopback: FAIL rspi u32 timed out");
            failures += 1;
        }
    }

    if failures == 0 {
        info!("loopback: PASS");
    } else {
        error!("loopback: FAIL ({} check(s))", failures);
    }
}

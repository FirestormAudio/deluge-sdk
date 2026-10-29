//! Runtime switching between USB device faces (spec §5.6, "Device side"): the Overbridge 2
//! class, or a class-compliant UAC2 audio device. Switching drops the `UsbDevice`, disconnects
//! (D+ pull-up off, so the host sees an unplug), and builds the other face's descriptors on the
//! same port, which the host then enumerates as a new device.
//!
//! One task owns the port and services the face's endpoints itself, so nothing outlives a face:
//! its descriptor buffers, class state and endpoints all live in one loop iteration.

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_futures::join::join;
use embassy_futures::select::select;
use embassy_time::Timer;
use log::info;
use rza1_hal::Peri;
use rza1_hal::peripherals::USB0;
use rza1_hal::usb::{disconnect, init_device_mode};
use usb_classes::audio::AudioClass;
use usb_classes::overbridge::device::{self, OverbridgeConfig, State};
use usb_classes::overbridge::proto::Format;

use super::overbridge::{CH_FROM_HOST, CH_TO_HOST, receive_forever, send_forever};
use crate::UsbIrqs;

/// What the device presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Face {
    Overbridge = 0,
    Uac = 1,
}

static CURRENT: AtomicU8 = AtomicU8::new(Face::Overbridge as u8);

/// The face presented now.
pub(crate) fn current() -> Face {
    if CURRENT.load(Ordering::Relaxed) == Face::Uac as u8 {
        Face::Uac
    } else {
        Face::Overbridge
    }
}

/// Seconds each face stays up under `face-switch`.
const FACE_SECONDS: u64 = 30;

/// pid.codes test IDs, one PID per descriptor shape (a host caches descriptors by VID:PID, so
/// two shapes must not share one); development only, never distributed.
#[cfg(not(feature = "elektron-id"))]
const OVERBRIDGE_ID: (u16, u16) = (0x1209, 0x0001);
/// Elektron's VID with a PID no device table lists (spec §7.2; bench only).
#[cfg(feature = "elektron-id")]
const OVERBRIDGE_ID: (u16, u16) = (0x1935, 0x0BF0);
const UAC_ID: (u16, u16) = (0x1209, 0x0002);

fn config(id: (u16, u16), product: &'static str) -> embassy_usb::Config<'static> {
    let mut c = embassy_usb::Config::new(id.0, id.1);
    c.manufacturer = Some("Deluge SDK");
    c.product = Some(product);
    c.device_class = 0xEF;
    c.device_sub_class = 0x02;
    c.device_protocol = 0x01;
    c.composite_with_iads = true;
    c.self_powered = true;
    c.max_power = 0;
    c
}

/// Present each face in turn (or only the Overbridge face, without `face-switch`).
#[embassy_executor::task]
pub(crate) async fn faces_task(mut usb0: Peri<'static, USB0>) {
    let mut face = Face::Overbridge;
    loop {
        CURRENT.store(face as u8, Ordering::Relaxed);
        info!("usb: presenting the {:?} face", face);
        let (_port, driver) = init_device_mode(usb0.reborrow(), UsbIrqs);
        let mut cfg_desc = [0u8; 512];
        let mut bos_desc = [0u8; 64];
        let mut msos_desc = [0u8; 0];
        let mut control_buf = [0u8; 64];
        // How long this face stays up: until the next switch, or for good.
        let hold = async {
            if cfg!(feature = "face-switch") {
                Timer::after_secs(FACE_SECONDS).await
            } else {
                core::future::pending::<()>().await
            }
        };
        match face {
            Face::Overbridge => {
                let mut state = State::new();
                let mut builder = embassy_usb::Builder::new(
                    driver,
                    config(OVERBRIDGE_ID, "Deluge Overbridge test"),
                    &mut cfg_desc,
                    &mut bos_desc,
                    &mut msos_desc,
                    &mut control_buf,
                );
                let (sender, receiver, shared) = device::new(
                    &mut builder,
                    &mut state,
                    OverbridgeConfig {
                        format: Format::V2,
                        ch_to_host: CH_TO_HOST as u8,
                        ch_from_host: CH_FROM_HOST as u8,
                        name: "Deluge",
                    },
                );
                // USB-MIDI on interfaces 4-5, as on the Digitakt. Its endpoints aren't serviced.
                let _midi = embassy_usb::class::midi::MidiClass::new(&mut builder, 1, 1, 512);
                let mut usb = builder.build();
                let streams = join(send_forever(sender), receive_forever(receiver));
                let _ = select(select(usb.run(), streams), hold).await;
                let name = shared.host_name();
                let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                info!(
                    "usb: Overbridge face down (host named itself {:?})",
                    core::str::from_utf8(&name[..len]).unwrap_or("?")
                );
            }
            Face::Uac => {
                // Declared before the builder: the device holds a reference to it as a handler.
                let mut audio_slot = None;
                let mut builder = embassy_usb::Builder::new(
                    driver,
                    config(UAC_ID, "Deluge UAC2 test"),
                    &mut cfg_desc,
                    &mut bos_desc,
                    &mut msos_desc,
                    &mut control_buf,
                );
                let (audio, _ep_out, _ep_in) = AudioClass::<2>::new(&mut builder, 288);
                builder.handler(audio_slot.insert(audio));
                let mut usb = builder.build();
                // The audio endpoints aren't serviced: this face only proves the switch.
                let _ = select(usb.run(), hold).await;
                info!("usb: UAC face down");
            }
        }
        // Everything this face built has dropped; the host sees an unplug, then the next face.
        disconnect(usb0.reborrow());
        Timer::after_millis(500).await;
        if cfg!(feature = "face-switch") {
            face = match face {
                Face::Overbridge => Face::Uac,
                Face::Uac => Face::Overbridge,
            };
        }
    }
}

//! The Deluge's USB **host** role: the bus supervisor and the hosted-MIDI device registry.
//!
//! The class drivers it runs (USB-MIDI, UAC) are board-agnostic and live in
//! `usb_classes::host`. This module wires them to the RUSB1 host driver in
//! [`rza1l_hal::usb`] and to the Deluge's pipe budget and engine sample rate.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

/// Maximum concurrently hosted MIDI devices.
///
/// Bounded by the RUSB1 host pipe budget. RX pipes are dedicated per device
/// (USB is host-polled, so a receive must be armed to catch unsolicited MIDI).
/// With bulk pipe 1 reserved for a shared TX pipe, bulk RX has pipes 2-5 —
/// **4 devices**. The HAL does not yet multiplex OUT pipes, so TX currently
/// takes a dedicated pipe per device too and the practical ceiling is lower
/// (~2 bidirectional devices).
///
/// The C firmware reaches 6 by also servicing MIDI devices that use
/// *interrupt* endpoints on RX pipes 7-8 (`USB_CFG_HMIDI_INT_RECV_MIN/MAX`).
/// [`midi::find_midi_interface`] only claims bulk endpoints, so those two are
/// not available here. TODO: support interrupt-endpoint MIDI.
pub const MAX_MIDI_DEVICES: usize = 4;

/// Depth of the merged RX channel, in event packets.
const MIDI_RX_DEPTH: usize = 64;

/// Depth of each per-device TX channel, in event packets.
const MIDI_TX_DEPTH: usize = 16;

/// Number of device slots. Addresses run 1..=5 (`HCD_MAX_DEV` is 6), so index
/// by address directly and size for the widest address the HAL allows.
const MIDI_SLOTS: usize = 6;

/// Identifies a hosted device.
///
/// Not the raw USB address: addresses are freed and reused on hot-plug, so a
/// bare address could alias a different device. The generation counter makes
/// stale ids detectable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeviceId {
    slot: u8,
    generation: u16,
}

impl DeviceId {
    pub(crate) const fn new(slot: u8, generation: u16) -> Self {
        Self { slot, generation }
    }

    /// Device slot index (the USB address, `1..MIDI_SLOTS`).
    pub const fn slot(&self) -> u8 {
        self.slot
    }

    /// Generation counter — bumped each time a slot is reused.
    pub const fn generation(&self) -> u16 {
        self.generation
    }
}

/// Merged RX stream from every hosted MIDI device, tagged by source.
static MIDI_RX: Channel<CriticalSectionRawMutex, (DeviceId, [u8; 4]), MIDI_RX_DEPTH> =
    Channel::new();

/// Per-device TX queues, indexed by [`DeviceId::slot`].
///
/// `MidiHost` is owned by its device task and cannot be shared, so this channel
/// is the only route to a device's OUT endpoint.
static MIDI_TX: [Channel<CriticalSectionRawMutex, [u8; 4], MIDI_TX_DEPTH>; MIDI_SLOTS] =
    [const { Channel::new() }; MIDI_SLOTS];

/// Non-blocking read of the next hosted-MIDI packet, if any.
///
/// Mirrors the device-side idiom (`usb_classes::midi::try_recv_from_host`).
pub fn try_recv_midi() -> Option<(DeviceId, [u8; 4])> {
    MIDI_RX.try_receive().ok()
}

/// Await the next hosted-MIDI packet.
pub async fn recv_midi() -> (DeviceId, [u8; 4]) {
    MIDI_RX.receive().await
}

/// A send handle for one hosted device.
///
/// Obtained from [`midi_handle`]. Cheap and `Copy` — it is just an id.
#[derive(Clone, Copy, Debug)]
pub struct MidiHandle {
    id: DeviceId,
}

impl MidiHandle {
    /// The device this handle addresses.
    pub const fn id(&self) -> DeviceId {
        self.id
    }

    /// Queue a packet for the device, awaiting space if the queue is full.
    pub async fn send(&self, packet: [u8; 4]) {
        MIDI_TX[self.id.slot() as usize].send(packet).await;
    }

    /// Queue a packet, returning it if the queue is full.
    ///
    /// Prefer this on the audio path — never block audio on USB.
    pub fn try_send(&self, packet: [u8; 4]) -> Result<(), [u8; 4]> {
        MIDI_TX[self.id.slot() as usize]
            .try_send(packet)
            .map_err(|e| match e {
                embassy_sync::channel::TrySendError::Full(p) => p,
            })
    }
}

/// A send handle for `id`.
///
/// The handle does not verify the device is still attached: if it has detached,
/// packets queue and are drained when the slot is reused. Compare
/// [`DeviceId::generation`] against a freshly observed id to detect staleness.
pub const fn midi_handle(id: DeviceId) -> MidiHandle {
    MidiHandle { id }
}

/// The sample rate the UAC host asks hosted audio devices for: the codec's 44.1 kHz unless
/// [`set_uac_sample_rate`] chose another. A device that cannot take it runs at its own rate
/// and the class driver's resampler converts.
#[cfg(feature = "usb-host-uac")]
static UAC_SAMPLE_RATE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(44_100);

/// Choose the rate the UAC host streams at, for devices bound from now on. Set it to the
/// device's rate (48 kHz, say) when the SCUX converts to the codec's rate downstream, so the
/// class driver only trims clock drift.
#[cfg(feature = "usb-host-uac")]
pub fn set_uac_sample_rate(hz: u32) {
    UAC_SAMPLE_RATE.store(hz, core::sync::atomic::Ordering::Relaxed);
}

/// The rate the UAC host streams at (see [`set_uac_sample_rate`]).
#[cfg(feature = "usb-host-uac")]
pub fn uac_sample_rate() -> u32 {
    UAC_SAMPLE_RATE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Whether the configuration descriptor `config` (the whole chain, as read from the device)
/// declares a hub interface (class 0x09).
pub fn has_hub_interface(config: &[u8]) -> bool {
    const INTERFACE: u8 = 4;
    const HUB_CLASS: u8 = 9;
    let mut rest = config;
    // Each descriptor starts with bLength and bDescriptorType; stop at a zero or overrunning
    // length rather than looping on it or reading past the end.
    while let [len, kind, ..] = *rest {
        let len = len as usize;
        if len < 2 || len > rest.len() {
            return false;
        }
        if kind == INTERFACE && len >= 6 && rest[5] == HUB_CLASS {
            return true;
        }
        rest = &rest[len..];
    }
    false
}

/// Samples in the Overbridge receive ring: 512 frames at 42 channels (a Digitakt II), more
/// at fewer (2730 at a Digitakt's 12). 84 KB.
#[cfg(feature = "usb-host-overbridge")]
pub const OVERBRIDGE_RX_SAMPLES: usize = 42 * 512;
/// Samples in the Overbridge send ring: 512 frames at 8 channels. 16 KB.
#[cfg(feature = "usb-host-overbridge")]
pub const OVERBRIDGE_TX_SAMPLES: usize = 8 * 512;

/// The app's view of a hosted Overbridge device.
#[cfg(feature = "usb-host-overbridge")]
pub type OverbridgeStems =
    usb_classes::overbridge::stems::OverbridgeHandle<OVERBRIDGE_RX_SAMPLES, OVERBRIDGE_TX_SAMPLES>;

#[cfg(feature = "usb-host-overbridge")]
static OVERBRIDGE: OverbridgeStems = OverbridgeStems::new();

/// The hosted Overbridge device's stems: [`info`](OverbridgeStems::info) is `None` until one
/// is connected.
#[cfg(feature = "usb-host-overbridge")]
pub fn overbridge() -> &'static OverbridgeStems {
    &OVERBRIDGE
}

/// Which audio class driver binds a root-port device (spec §5.6). Change it any time; with a
/// device attached the supervisor re-binds it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostAudioMode {
    /// Overbridge when the device matches it, UAC otherwise.
    #[default]
    Auto = 0,
    /// Only UAC.
    Uac = 1,
    /// Only Overbridge.
    Overbridge = 2,
    /// No audio driver; MIDI still binds.
    Off = 3,
}

/// An audio class driver the supervisor can bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDriver {
    Uac,
    Overbridge,
}

static HOST_AUDIO_MODE: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(HostAudioMode::Auto as u8);

/// Set the host audio mode. With a device attached, it is re-bound under the new mode.
pub fn set_host_audio_mode(mode: HostAudioMode) {
    if HOST_AUDIO_MODE.swap(mode as u8, core::sync::atomic::Ordering::AcqRel) != mode as u8 {
        #[cfg(target_os = "none")]
        runtime::MODE_CHANGED.signal(());
    }
}

/// The current host audio mode.
pub fn host_audio_mode() -> HostAudioMode {
    match HOST_AUDIO_MODE.load(core::sync::atomic::Ordering::Acquire) {
        1 => HostAudioMode::Uac,
        2 => HostAudioMode::Overbridge,
        3 => HostAudioMode::Off,
        _ => HostAudioMode::Auto,
    }
}

/// The audio drivers to try for a device with vendor ID `vid` under `mode`, in order, before
/// the build's features filter them ([`compiled`]).
pub fn audio_drivers(mode: HostAudioMode, vid: u16) -> &'static [AudioDriver] {
    const ELEKTRON: u16 = 0x1935;
    match mode {
        HostAudioMode::Auto if vid == ELEKTRON => &[AudioDriver::Overbridge, AudioDriver::Uac],
        HostAudioMode::Auto | HostAudioMode::Uac => &[AudioDriver::Uac],
        HostAudioMode::Overbridge => &[AudioDriver::Overbridge],
        HostAudioMode::Off => &[],
    }
}

/// Whether this build includes `d`.
pub fn compiled(d: AudioDriver) -> bool {
    match d {
        AudioDriver::Uac => cfg!(feature = "usb-host-uac"),
        AudioDriver::Overbridge => cfg!(feature = "usb-host-overbridge"),
    }
}

#[cfg(target_os = "none")]
mod runtime {
    use core::sync::atomic::{AtomicU16, Ordering};

    use embassy_executor::Spawner;
    use embassy_futures::select::{Either, select};
    use embassy_usb_driver::host::DeviceEvent;
    use embassy_usb_host::class::hub::{HubEvent, HubHandler};
    use embassy_usb_host::descriptor::ConfigurationDescriptor;
    use embassy_usb_host::handler::{BusRoute, HandlerEvent, RegisterError};
    use embassy_usb_host::{BusHandle, BusState, bus};
    use log::{error, info, warn};
    use rza1l_hal::usb::{Rusb1Allocator, Rusb1HostDriver};

    use super::{DeviceId, MAX_MIDI_DEVICES, MIDI_RX, MIDI_TX};
    use usb_classes::host::midi::MidiHost;

    /// Raised by [`super::set_host_audio_mode`]; the supervisor re-binds an attached device.
    pub(super) static MODE_CHANGED: embassy_sync::signal::Signal<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        (),
    > = embassy_sync::signal::Signal::new();

    /// Log, once, that the host audio mode names a driver this build leaves out.
    fn warn_once_missing(mode: super::HostAudioMode) {
        static WARNED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            warn!(
                "usb_host: host audio mode {:?} needs a driver this build doesn't include",
                mode
            );
        }
    }

    /// The allocator the class drivers are actually parameterised by.
    ///
    /// `BusHandle` implements `UsbHostAllocator` by forwarding to the inner
    /// `Rusb1Allocator`, and it is what `bus()` hands back — so this, not
    /// `Rusb1Allocator`, is the type that flows into the class drivers.
    type HostAlloc = BusHandle<'static, Rusb1Allocator>;

    /// Pause before retrying a device whose enumeration failed.
    const ENUMERATION_RETRY_MS: u64 = 500;

    /// Maximum hub ports serviced.
    ///
    /// The RUSB1 `DEVADDn.HUBPORT` field is 3 bits (TRM §28.3), so a device can
    /// only be addressed on hub ports 1-7.
    const MAX_HUB_PORTS: usize = 7;

    /// Monotonic [`DeviceId`] generation counter.
    ///
    /// Shared between the supervisor and every hub task: both key ids by USB
    /// address, and an address freed by one can be reissued to the other, so a
    /// per-task counter could mint colliding ids for different devices.
    static GENERATION: AtomicU16 = AtomicU16::new(0);

    /// Next generation for a freshly bound device.
    fn next_generation() -> u16 {
        GENERATION.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    /// Bind an enumerated device's MIDI interface and spawn its task.
    ///
    /// Returns `true` if the device was claimed. On `false` the caller owns
    /// freeing the address.
    fn bind_midi(
        handle: &HostAlloc,
        spawner: Spawner,
        dev_info: &embassy_usb_host::handler::EnumerationInfo,
        cfg: &ConfigurationDescriptor<'_>,
    ) -> bool {
        let addr = dev_info.device_address;
        let id = DeviceId::new(addr, next_generation());

        match MidiHost::try_register(handle, addr, dev_info.split(), cfg) {
            Ok(host) => {
                info!(
                    "usb_host: MIDI device VID={:04x} PID={:04x} addr={}",
                    dev_info.device_desc.vendor_id, dev_info.device_desc.product_id, addr
                );
                // In embassy-executor 0.10 the task fn returns the token as a
                // Result (pool exhaustion), while `Spawner::spawn` returns ().
                // Reaching the Err arm means more devices than MAX_MIDI_DEVICES.
                match midi_device_task(host, id) {
                    Ok(token) => {
                        spawner.spawn(token);
                        true
                    }
                    Err(_) => {
                        error!("usb_host: no free device task slot");
                        false
                    }
                }
            }
            Err(e) => {
                // The caller frees the address, so an unsupported device does
                // not consume one of the scarce device slots.
                warn!("usb_host: unsupported device: {:?}", e);
                false
            }
        }
    }

    /// Bind an enumerated device's UAC2 capture interface and spawn its task.
    ///
    /// Returns `true` if the device was claimed. On `false` the caller owns
    /// freeing the address. Mirrors [`bind_midi`]: registration is synchronous
    /// (via `block_on`) because `try_register`'s control transfers are short
    /// and the supervisor is already the enumeration owner.
    #[cfg(feature = "usb-host-uac")]
    fn bind_uac(
        handle: &HostAlloc,
        spawner: Spawner,
        dev_info: &embassy_usb_host::handler::EnumerationInfo,
        cfg: &ConfigurationDescriptor<'_>,
    ) -> bool {
        use usb_classes::host::uac::Uac;

        let addr = dev_info.device_address;
        match embassy_futures::block_on(Uac::try_register(
            handle,
            addr,
            dev_info.split(),
            cfg,
            super::uac_sample_rate(),
        )) {
            Ok(host) => {
                info!(
                    "usb_host: UAC capture device VID={:04x} PID={:04x} addr={} channels={}",
                    dev_info.device_desc.vendor_id,
                    dev_info.device_desc.product_id,
                    addr,
                    host.channels()
                );
                // Capture the channel count before `host` moves into the
                // task, and only call `shared::begin` once the spawn actually
                // succeeds. If a second concurrent UAC device hits a full task
                // pool, `begin` must not run: it resets the shared ring and
                // overwrites `channels`, corrupting the state of the first
                // device, which is still streaming.
                let ch = host.channels();
                let play_ch = host.playback_channels();
                match uac_capture_task(host) {
                    Ok(token) => {
                        spawner.spawn(token);
                        usb_classes::host::uac::shared::begin(ch);
                        if play_ch > 0 {
                            usb_classes::host::uac::shared::playback_begin(play_ch);
                        }
                        true
                    }
                    Err(_) => {
                        error!("usb_host: could not spawn UAC capture task");
                        false
                    }
                }
            }
            Err(e) => {
                warn!("usb_host: unsupported UAC device: {:?}", e);
                false
            }
        }
    }

    /// Services one hosted UAC2 capture device until it errors or detaches.
    ///
    /// Owns the pipes, so they drop — and the hardware pipes free — the moment
    /// this exits. Publishes decoded frames into [`usb_classes::host::uac::shared`], the
    /// cross-task bridge the engine drains via `capture_read`.
    #[cfg(feature = "usb-host-uac")]
    #[embassy_executor::task]
    async fn uac_capture_task(mut host: usb_classes::host::uac::Uac<'static, HostAlloc>) {
        // BadResponse is the HAL's detach signal AND a transient iso-fault signal.
        // Absorb transient faults; treat only sustained failure as a real detach.
        // 24 consecutive microframe failures ~= 3 ms of no data, far beyond any
        // single-packet glitch but a prompt exit on a real unplug.
        const DETACH_THRESHOLD: u32 = 24;
        let mut consecutive_errs: u32 = 0;
        loop {
            match host.pump_once_shared().await {
                Ok(()) => consecutive_errs = 0,
                Err(_) => {
                    consecutive_errs += 1;
                    if consecutive_errs >= DETACH_THRESHOLD {
                        break;
                    }
                }
            }
        }
        usb_classes::host::uac::shared::end();
        usb_classes::host::uac::shared::playback_end();
        // `host` drops here → pipes drop → address reclaimed.
    }

    /// Bind an Elektron device's Overbridge interfaces and spawn its stream task. `false` when
    /// it doesn't match (an Elektron box in Audio/MIDI mode) or setup fails twice.
    #[cfg(feature = "usb-host-overbridge")]
    async fn bind_overbridge(
        handle: &HostAlloc,
        spawner: Spawner,
        dev_info: &embassy_usb_host::handler::EnumerationInfo,
        cfg: &ConfigurationDescriptor<'_>,
    ) -> bool {
        use usb_classes::host::overbridge::{OverbridgeError, OverbridgeHost};
        let d = &dev_info.device_desc;
        let register = || {
            OverbridgeHost::try_register(
                handle,
                dev_info.device_address,
                dev_info.split(),
                d.vendor_id,
                d.product_id,
                cfg,
            )
        };
        // Spec §5.5: a failed SET_INTERFACE is retried once after 100 ms. Awaited, not
        // `block_on`, so the rest of the firmware keeps running meanwhile.
        let host = match register().await {
            Err(OverbridgeError::SetupFailed(e)) => {
                warn!("usb_host: overbridge setup failed ({:?}); retrying", e);
                embassy_time::Timer::after_millis(100).await;
                register().await
            }
            r => r,
        };
        let host = match host {
            Ok(h) => h,
            Err(OverbridgeError::NotOverbridge) => return false,
            Err(e) => {
                warn!("usb_host: overbridge device not bound: {:?}", e);
                return false;
            }
        };
        let m = *host.matched();
        if super::overbridge()
            .begin(m.info(), m.ch_from, m.ch_to)
            .is_err()
        {
            warn!(
                "usb_host: overbridge {} has more channels than the handle holds",
                m.name
            );
            return false;
        }
        match overbridge_task(host) {
            Ok(token) => {
                spawner.spawn(token);
                true
            }
            Err(_) => {
                super::overbridge().end();
                error!("usb_host: could not spawn the overbridge task");
                false
            }
        }
    }

    /// Streams one Overbridge device until it detaches or keeps failing: a transport error
    /// restarts the stream (spec §5.5), and the third within 10 s ends it.
    #[cfg(feature = "usb-host-overbridge")]
    #[embassy_executor::task]
    async fn overbridge_task(
        mut host: usb_classes::host::overbridge::OverbridgeHost<'static, HostAlloc>,
    ) {
        use embassy_time::{Duration, Instant, Timer};
        const WINDOW: Duration = Duration::from_secs(10);
        let mut failures: heapless::Deque<Instant, 3> = heapless::Deque::new();
        use usb_classes::host::overbridge::OverbridgeError;
        let stems = super::overbridge();
        // Prime the OUT stream here, not in the supervisor: a device that holds off OUT until
        // it streams IN would otherwise stall enumeration.
        if let Err(e) = host.start().await {
            warn!("usb_host: overbridge stream did not start: {:?}", e);
            stems.end();
            return;
        }
        loop {
            let Err(e) = host.pump_once(stems, Instant::now().as_micros()).await else {
                continue;
            };
            let now = Instant::now();
            while failures
                .front()
                .is_some_and(|t| now.duration_since(*t) > WINDOW)
            {
                failures.pop_front();
            }
            if failures.is_full() {
                failures.pop_front();
            }
            let _ = failures.push_back(now);
            warn!(
                "usb_host: overbridge stream error {:?} ({} in 10 s)",
                e,
                failures.len()
            );
            if failures.len() >= 3 {
                break;
            }
            // Spec §5.5: re-run setup; a failed SET_INTERFACE is retried once after 100 ms.
            let restarted = match host.restart().await {
                Err(OverbridgeError::SetupFailed(_)) => {
                    Timer::after_millis(100).await;
                    host.restart().await
                }
                r => r,
            };
            if restarted.is_err() {
                break;
            }
        }
        info!("usb_host: overbridge stream ended");
        stems.end();
    }

    /// Bytes read of a device's whole configuration descriptor. Audio interfaces carry long
    /// ones (alternate settings, clock and terminal units): a USB-C UAC2 interface on hardware
    /// had 554 bytes, past the 512 this used to be, and failed enumeration.
    const CONFIG_DESC_BUF: usize = 2048;

    /// Owns a hub: services port changes and enumerates devices behind it.
    ///
    /// Enumeration lives here rather than in the supervisor because
    /// [`HubHandler::enumerate_port`] needs `&mut HubHandler`. It also performs
    /// the port reset, the USB 2.0 §7.1.7.5 delay, and computes the correct
    /// `BusRoute`/`SplitInfo` (including parent-hub TT handling) — none of that
    /// is hand-rolled here.
    #[embassy_executor::task]
    async fn hub_task(
        mut hub: HubHandler<'static, Rusb1Allocator, MAX_HUB_PORTS>,
        handle: HostAlloc,
        spawner: Spawner,
    ) {
        let mut config_buf = [0u8; CONFIG_DESC_BUF];

        loop {
            let event = match hub.wait_for_event().await {
                Ok(HandlerEvent::HandlerEvent(e)) => e,
                Ok(HandlerEvent::NoChange) => continue,
                Ok(HandlerEvent::HandlerDisconnected) => {
                    info!("usb_host: hub disconnected");
                    return;
                }
                Err(e) => {
                    error!("usb_host: hub error: {:?}", e);
                    return;
                }
            };

            match event {
                HubEvent::DeviceDetected { port, speed } => {
                    info!("usb_host: hub port {} attached ({:?})", port, speed);
                    let (dev_info, _) = match hub.enumerate_port(&mut config_buf, port, speed).await
                    {
                        Ok(v) => v,
                        Err(e) => {
                            error!("usb_host: hub port {} enumeration failed: {:?}", port, e);
                            continue;
                        }
                    };
                    let addr = dev_info.device_address;

                    let bound = match ConfigurationDescriptor::try_from_slice(&config_buf) {
                        Ok(cfg) => bind_midi(&handle, spawner, &dev_info, &cfg),
                        Err(e) => {
                            error!("usb_host: hub port {} bad descriptor: {:?}", port, e);
                            false
                        }
                    };
                    if !bound {
                        handle.free_address(addr);
                    }
                }
                HubEvent::DeviceRemoved { address, port } => {
                    info!("usb_host: hub port {} detached", port);
                    // The device task exits on its next failed read, dropping
                    // its pipes. Only the address needs reclaiming here.
                    if let Some(a) = address {
                        handle.free_address(a.get());
                    }
                }
            }
        }
    }

    /// Services one hosted MIDI device until it errors or detaches.
    ///
    /// Owns the pipes, so they drop — and the hardware pipes free — the moment
    /// this exits. That is the whole detach cleanup path.
    ///
    /// Reads and writes are serviced together via `select`: the device's TX
    /// channel is the only route to its OUT endpoint, since `MidiHost` lives
    /// here and cannot be shared.
    #[embassy_executor::task(pool_size = MAX_MIDI_DEVICES)]
    async fn midi_device_task(mut host: MidiHost<'static, HostAlloc>, id: DeviceId) {
        let tx = &MIDI_TX[id.slot() as usize];
        let mut packets = [[0u8; 4]; 16];

        loop {
            if !host.can_receive() {
                // Sink-only device (a synth): only ever send.
                let p = tx.receive().await;
                if let Err(e) = host.write(&[p]).await {
                    warn!("usb_host: device {:?} write error: {:?}", id, e);
                }
                continue;
            }

            match select(host.read(&mut packets), tx.receive()).await {
                Either::First(Ok(n)) => {
                    for p in &packets[..n] {
                        // Drop rather than block: a stalled consumer must not
                        // stall USB.
                        let _ = MIDI_RX.try_send((id, *p));
                    }
                }
                Either::First(Err(e)) => {
                    info!("usb_host: device {:?} read ended: {:?}", id, e);
                    return;
                }
                Either::Second(p) => {
                    if let Err(e) = host.write(&[p]).await {
                        // A send error is normal when a device attaches or
                        // detaches from a hub mid-traffic — the C
                        // firmware logs and continues rather than dropping the
                        // device (midi_engine.cpp:128-135). Do the same.
                        warn!("usb_host: device {:?} write error: {:?}", id, e);
                    }
                }
            }
        }
    }

    /// Owns the bus: waits for attach, enumerates, binds a class driver.
    ///
    /// Spawn this instead of a bare enumerate-and-log task when the port is in
    /// host mode. The `driver` must have been initialised via
    /// [`rza1l_hal::usb::init_host_mode`], whose binding serves host mode.
    #[embassy_executor::task]
    pub async fn usb_host_supervisor(driver: Rusb1HostDriver<'static>, spawner: Spawner) {
        static BUS_STATE: BusState = BusState::new();
        let (mut controller, handle) = bus(driver, &BUS_STATE);
        let mut config_buf = [0u8; CONFIG_DESC_BUF];

        loop {
            let speed = controller.wait_for_connection().await;
            info!("usb_host: device connected ({:?})", speed);

            let (dev_info, _) = match handle
                .enumerate(BusRoute::Direct(speed), &mut config_buf)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    error!("usb_host: enumeration failed: {:?}", e);
                    // The device stays on the line and is found again at once; pace
                    // the retries so one that cannot enumerate is not reset in a
                    // tight loop.
                    embassy_time::Timer::after_millis(ENUMERATION_RETRY_MS).await;
                    continue;
                }
            };
            let addr = dev_info.device_address;

            // Only a device declaring a hub interface goes to the hub driver:
            // HubHandler re-reads the configuration into its own fixed buffer
            // before it checks the class, so a device with a long configuration
            // (an audio interface's, 554 bytes on hardware) fails there with
            // InsufficientMemory instead of reaching the class matchers.
            let bound = if super::has_hub_interface(&config_buf) {
                match HubHandler::<Rusb1Allocator, MAX_HUB_PORTS>::try_register(&handle, &dev_info)
                    .await
                {
                    Ok(hub) => {
                        info!("usb_host: hub at addr={}", addr);
                        // `BusHandle` is Clone (allocator + &'static BusState), so
                        // handing the hub task its own clone is cheap.
                        match hub_task(hub, handle.clone(), spawner) {
                            Ok(token) => {
                                spawner.spawn(token);
                                true
                            }
                            Err(_) => {
                                error!("usb_host: could not spawn hub task");
                                false
                            }
                        }
                    }
                    Err(e) => {
                        error!("usb_host: hub register failed: {:?}", e);
                        false
                    }
                }
            } else {
                // A composite device (an audio interface with MIDI, the Digitakt
                // in AUDIO/MIDI mode) gets both drivers: MIDI claims its bulk
                // pipes, UAC its isochronous ones, and the address stays claimed
                // until detach.
                match ConfigurationDescriptor::try_from_slice(&config_buf) {
                    Ok(cfg) => {
                        let midi = bind_midi(&handle, spawner, &dev_info, &cfg);
                        let mode = super::host_audio_mode();
                        let vid = dev_info.device_desc.vendor_id;
                        let mut audio = false;
                        for d in super::audio_drivers(mode, vid) {
                            if !super::compiled(*d) {
                                warn_once_missing(mode);
                                continue;
                            }
                            audio = match d {
                                #[cfg(feature = "usb-host-overbridge")]
                                super::AudioDriver::Overbridge => {
                                    bind_overbridge(&handle, spawner, &dev_info, &cfg).await
                                }
                                #[cfg(feature = "usb-host-uac")]
                                super::AudioDriver::Uac => {
                                    bind_uac(&handle, spawner, &dev_info, &cfg)
                                }
                                #[allow(unreachable_patterns)]
                                _ => false,
                            };
                            if audio {
                                break;
                            }
                        }
                        midi || audio
                    }
                    Err(e) => {
                        error!("usb_host: bad config descriptor: {:?}", e);
                        false
                    }
                }
            };

            if !bound {
                handle.free_address(addr);
                continue;
            }

            // Wait for detach before accepting another root-port device. A hub
            // reaches here too, so unplugging one is noticed.
            loop {
                if controller.wait_for_device_event().await == DeviceEvent::Disconnected {
                    info!("usb_host: device disconnected");
                    handle.free_address(addr);
                    break;
                }
            }
        }
    }
}

#[cfg(target_os = "none")]
pub use runtime::usb_host_supervisor;

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// A configuration descriptor (9 bytes) followed by one interface descriptor per class.
    fn config_with_interfaces(classes: &[u8]) -> std::vec::Vec<u8> {
        let mut v = std::vec![9, 2, 0, 0, classes.len() as u8, 1, 0, 0x80, 50];
        for (i, &class) in classes.iter().enumerate() {
            v.extend_from_slice(&[9, 4, i as u8, 0, 1, class, 0, 0, 0]);
            v.extend_from_slice(&[7, 5, 0x81, 3, 8, 0, 12]); // an endpoint between interfaces
        }
        let total = v.len() as u16;
        v[2..4].copy_from_slice(&total.to_le_bytes());
        v
    }

    #[cfg(feature = "usb-host-uac")]
    #[test]
    fn the_uac_rate_defaults_to_the_codec_rate_and_can_be_chosen() {
        assert_eq!(uac_sample_rate(), 44_100);
        set_uac_sample_rate(48_000);
        assert_eq!(uac_sample_rate(), 48_000);
        set_uac_sample_rate(44_100);
    }

    #[test]
    fn an_audio_interface_configuration_has_no_hub_interface() {
        // Audio control, audio streaming, MIDI: the classes an audio interface carries.
        assert!(!has_hub_interface(&config_with_interfaces(&[1, 1, 1])));
    }

    #[test]
    fn a_hub_configuration_has_a_hub_interface() {
        assert!(has_hub_interface(&config_with_interfaces(&[9])));
    }

    #[test]
    fn a_malformed_configuration_has_no_hub_interface() {
        let mut zero_len = config_with_interfaces(&[1]);
        zero_len[9] = 0; // a zero bLength would otherwise never advance
        assert!(!has_hub_interface(&zero_len));
        let mut overrun = config_with_interfaces(&[9]);
        overrun[9] = 200; // the interface descriptor claims to run past the end
        assert!(!has_hub_interface(&overrun));
        assert!(!has_hub_interface(&[]));
    }

    #[test]
    fn device_ids_in_the_same_slot_differ_across_generations() {
        // A USB address is freed and reused on hot-plug. If DeviceId were just
        // the address, a stale id would silently alias a different controller.
        let a = DeviceId::new(2, 7);
        let b = DeviceId::new(2, 8);
        assert_ne!(a, b, "same slot, later generation must not compare equal");
        assert_eq!(a.slot(), b.slot());
    }

    #[test]
    fn device_ids_are_equal_within_a_generation() {
        assert_eq!(DeviceId::new(3, 1), DeviceId::new(3, 1));
    }

    #[test]
    fn handle_try_send_routes_to_the_devices_own_queue() {
        let h = midi_handle(DeviceId::new(2, 1));
        h.try_send([0x09, 0x90, 60, 100]).expect("queue has space");

        assert_eq!(
            MIDI_TX[2].try_receive().ok(),
            Some([0x09, 0x90, 60, 100]),
            "packet must land in slot 2's queue"
        );
        assert!(
            MIDI_TX[3].try_receive().is_err(),
            "no other device's queue may be touched"
        );
    }

    #[test]
    fn handle_try_send_reports_a_full_queue_instead_of_blocking() {
        let h = midi_handle(DeviceId::new(4, 1));
        for _ in 0..MIDI_TX_DEPTH {
            h.try_send([0x0F, 0xF8, 0, 0]).expect("fills to capacity");
        }
        // The audio path must never block on USB backpressure.
        assert_eq!(h.try_send([0x0F, 0xF8, 0, 0]), Err([0x0F, 0xF8, 0, 0]));
    }

    #[cfg(feature = "usb-host-overbridge")]
    #[test]
    fn the_overbridge_handle_holds_a_digitakt_ii() {
        use usb_classes::overbridge::stems::MIN_RING_FRAMES;
        assert!(
            OVERBRIDGE_RX_SAMPLES / 42 >= 512,
            "≥ 512 frames (10.7 ms) at 42 channels"
        );
        assert!(OVERBRIDGE_TX_SAMPLES / 8 >= 512);
        assert!(OVERBRIDGE_RX_SAMPLES / 42 >= MIN_RING_FRAMES);
    }

    #[test]
    fn auto_offers_elektron_devices_to_overbridge_first() {
        assert_eq!(
            audio_drivers(HostAudioMode::Auto, 0x1935),
            &[AudioDriver::Overbridge, AudioDriver::Uac]
        );
        assert_eq!(
            audio_drivers(HostAudioMode::Auto, 0x1234),
            &[AudioDriver::Uac]
        );
    }

    #[test]
    fn a_forced_mode_never_tries_the_other_driver() {
        assert_eq!(
            audio_drivers(HostAudioMode::Uac, 0x1935),
            &[AudioDriver::Uac]
        );
        assert_eq!(
            audio_drivers(HostAudioMode::Overbridge, 0x1234),
            &[AudioDriver::Overbridge]
        );
        assert!(audio_drivers(HostAudioMode::Off, 0x1935).is_empty());
    }

    #[test]
    fn a_mode_the_build_lacks_binds_no_audio() {
        let usable: std::vec::Vec<_> = audio_drivers(HostAudioMode::Overbridge, 0x1935)
            .iter()
            .filter(|d| compiled(**d))
            .collect();
        assert_eq!(usable.is_empty(), !cfg!(feature = "usb-host-overbridge"));
    }

    #[test]
    fn the_mode_round_trips_and_defaults_to_auto() {
        assert_eq!(HostAudioMode::default(), HostAudioMode::Auto);
        for m in [
            HostAudioMode::Off,
            HostAudioMode::Uac,
            HostAudioMode::Overbridge,
            HostAudioMode::Auto,
        ] {
            set_host_audio_mode(m);
            assert_eq!(host_audio_mode(), m);
        }
    }
}

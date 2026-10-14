//! Host builds only: a hook for a simulated front panel.
//!
//! On the device the PIC drives the pads and LEDs and the OLED is on SPI. On the host, the
//! peripheral modules' host arms call the [`HostPanel`] installed here instead: every outbound
//! PIC command ([`crate::pic`]), every OLED frame ([`crate::oled`]), and the PIC bytes the panel's
//! input would produce. With nothing installed they stay no-ops. The BSP knows nothing about
//! which simulator implements the hook; `deluge-sdk`'s `sim` backend installs one.

use std::sync::OnceLock;

/// A simulated front panel.
pub trait HostPanel: Sync {
    /// Show one OLED frame (`oled::FrameBuffer::as_bytes` layout).
    fn display(&self, frame: &[u8]);
    /// Apply one whole PIC command, as the PIC would.
    fn pic_command(&self, bytes: &[u8]);
    /// The next byte the PIC would send for the panel's input, if any is waiting.
    fn next_pic_byte(&self) -> Option<u8>;
}

/// Holds the one installed panel.
struct Slot(OnceLock<&'static dyn HostPanel>);

impl Slot {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    fn install(&self, panel: &'static dyn HostPanel) {
        if self.0.set(panel).is_err() {
            panic!("deluge_bsp::host_panel::install called twice");
        }
    }

    fn get(&self) -> Option<&'static dyn HostPanel> {
        self.0.get().copied()
    }
}

static SLOT: Slot = Slot::new();

/// Install the host panel. Call once, before the program starts.
///
/// # Panics
/// If a panel is already installed.
pub fn install(panel: &'static dyn HostPanel) {
    SLOT.install(panel);
}

/// The installed panel, if any.
pub fn installed() -> Option<&'static dyn HostPanel> {
    SLOT.get()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::vec::Vec;

    /// Records what the BSP sends it, and serves queued input bytes.
    #[derive(Default)]
    pub(crate) struct RecordingPanel {
        pub(crate) frames: Mutex<Vec<Vec<u8>>>,
        pub(crate) commands: Mutex<Vec<Vec<u8>>>,
        pub(crate) input: Mutex<Vec<u8>>,
    }

    impl HostPanel for RecordingPanel {
        fn display(&self, frame: &[u8]) {
            self.frames.lock().unwrap().push(frame.to_vec());
        }
        fn pic_command(&self, bytes: &[u8]) {
            self.commands.lock().unwrap().push(bytes.to_vec());
        }
        fn next_pic_byte(&self) -> Option<u8> {
            let mut input = self.input.lock().unwrap();
            (!input.is_empty()).then(|| input.remove(0))
        }
    }

    pub(crate) fn leak_panel() -> &'static RecordingPanel {
        Box::leak(Box::default())
    }

    #[test]
    fn slot_is_empty_until_installed() {
        let slot = Slot::new();
        assert!(slot.get().is_none());
        slot.install(leak_panel());
        assert!(slot.get().is_some());
    }

    #[test]
    #[should_panic(expected = "host_panel::install")]
    fn installing_twice_panics() {
        let slot = Slot::new();
        slot.install(leak_panel());
        slot.install(leak_panel());
    }
}

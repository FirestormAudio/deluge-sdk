//! Host (desktop-simulator) backend, active when an app is built for the host
//! triple by `cargo deluge sim` (`cfg(not(target_os = "none"))`).
//!
//! On the device the capability modules talk to `deluge-bsp` peripherals; on the
//! host they talk to a [`SharedPanel`] instead — the in-memory link the simulator
//! GUI renders from. The panel and the app side of the audio bridge are held by
//! `deluge_bsp::sim`, installed once by [`crate::__rt::host::run`] before any app
//! code runs, so the SDK and a program on the BSP directly share one copy.

use deluge_sim_link::SharedPanel;

/// The process-wide shared panel. Panics if called before the host runtime installs it.
pub(crate) fn panel() -> &'static SharedPanel {
    deluge_bsp::sim::panel().expect("host panel not initialised (run via `cargo deluge sim`)")
}

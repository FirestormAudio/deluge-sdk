//! Deluge desktop front-panel simulator.
//!
//! Renders the Deluge's OLED, RGB pad grid, buttons, encoders and LEDs and feeds
//! input back to a "brain" that owns all UI logic. Two front-ends share the same
//! rendering:
//!
//! - [`run_connected`] — drive an external brain over the [`deluge_protocol`]
//!   wire (TCP / Unix socket), e.g. DelugeFirmware's native `deluge_host`.
//! - [`run_in_process`] — drive an SDK app running in *this* process, sharing
//!   state through [`deluge_sim_link`] with no serialization. This is what
//!   `cargo deluge sim` uses.

mod app;
mod audio;
mod display;
mod hardware;
mod hardware_state;
mod link;
mod pad_grid;
mod renderer;
mod rgb;

use app::DelugeSimulator;
use deluge_sim_link::SharedPanel;
use deluge_sim_link::audio::GuiEnds;
use link::{InProcessLink, LinkKind, PanelLink};

/// The faceplate art, rasterised at launch.
const DELUGE_SVG: &[u8] = include_bytes!("../assets/Deluge.svg");

/// Rasterise the faceplate SVG to an iced image handle (half native resolution —
/// iced scales it to the window anyway).
fn render_svg() -> Option<iced::widget::image::Handle> {
    const WIDTH: u32 = 1089;
    const HEIGHT: u32 = 741;
    const SCALE: f32 = 0.5;
    let tree = usvg::Tree::from_data(DELUGE_SVG, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(WIDTH, HEIGHT)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(SCALE, SCALE), &mut pixmap.as_mut());
    Some(iced::widget::image::Handle::from_rgba(WIDTH, HEIGHT, pixmap.data().to_vec()))
}

/// Launch the iced window with the given link. Blocks until the window closes.
fn run_app(link: LinkKind) -> iced::Result {
    let svg = render_svg();
    // iced's init closure must be `Fn` (called once); hand the link + svg through
    // a Mutex<Option<_>> so the first call can `take()` them.
    let init = std::sync::Mutex::new(Some((link, svg)));
    iced::application(
        move || {
            let (link, svg) = init.lock().unwrap().take().expect("init called once");
            (DelugeSimulator::new(link, svg), iced::Task::none())
        },
        DelugeSimulator::update,
        DelugeSimulator::view,
    )
    .title(|s: &DelugeSimulator| s.title())
    .window_size(iced::Size::new(1089.0, 741.0)) // half the SVG's 2178×1482
    .resizable(false)
    .theme(|s: &DelugeSimulator| s.theme())
    .subscription(|s: &DelugeSimulator| s.subscription())
    .run()
}

/// Run the panel against an external brain over `deluge-protocol`. `target` is a
/// connect string (`host:port`, `:port`, `tcp://…`, or a Unix socket path); pass
/// `None` for a passive, blank panel.
pub fn run_connected(target: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let link = match target {
        Some(t) => {
            println!("deluge-simulator: connecting to brain at {t} ...");
            LinkKind::Protocol(PanelLink::connect(t)?)
        }
        None => {
            println!("deluge-simulator: no --connect target; opening a passive (blank) panel.");
            LinkKind::None
        }
    };
    run_app(link)?;
    Ok(())
}

/// Run the panel against an SDK app in the same process: start host audio, then
/// hand the main thread to the GUI. Returns when the window closes.
///
/// Called by the SDK's host runtime (`cargo deluge sim`). The `panel` carries
/// illumination + input; `gui_audio` carries the audio rings to the host's
/// speakers/mic.
pub fn run_in_process(panel: SharedPanel, gui_audio: GuiEnds) {
    // Kept alive until the window closes (dropping the streams stops audio).
    let _audio = audio::start(gui_audio);
    let link = LinkKind::InProcess(InProcessLink::new(panel));
    if let Err(e) = run_app(link) {
        eprintln!("deluge-simulator: {e}");
    }
}

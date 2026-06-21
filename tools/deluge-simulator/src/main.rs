//! Deluge desktop front-panel simulator.
//!
//! Renders the Deluge's OLED, RGB pad grid, buttons, encoders and LEDs, and forwards
//! input over the [`deluge_protocol`] wire to a "brain" that owns all UI logic. Point it
//! at DelugeFirmware's native `deluge_host` (whose `host_link` bridge listens on the
//! stream) and you have a true emulator: the real firmware runs, this is its panel.
//!
//! The brain can be reached over TCP loopback (works everywhere, including Windows) or a
//! Unix domain socket (Unix only):
//!
//! ```text
//! DELUGE_HOST_LINK=127.0.0.1:9000   ./build-sim/deluge_host   # the brain (TCP)
//! deluge-simulator --connect 127.0.0.1:9000                   # the panel
//!
//! DELUGE_HOST_LINK=/tmp/deluge.sock ./build-sim/deluge_host   # the brain (Unix socket)
//! deluge-simulator --connect /tmp/deluge.sock                 # the panel
//! ```

mod app;
mod display;
mod hardware;
mod hardware_state;
mod link;
mod pad_grid;
mod renderer;
mod rgb;

use app::DelugeSimulator;
use clap::Parser;
use link::PanelLink;

/// The faceplate art, rasterised at launch.
const DELUGE_SVG: &[u8] = include_bytes!("../assets/Deluge.svg");

#[derive(Parser)]
#[command(name = "deluge-simulator", about = "Desktop front panel for a Deluge brain")]
struct Cli {
    /// Connect to a Deluge brain (e.g. DelugeFirmware's `deluge_host`, launched with
    /// `DELUGE_HOST_LINK=<target>`). Accepts a TCP endpoint (`host:port`, `:port`, or
    /// `tcp://host:port`) on any platform, or a Unix domain socket path on Unix. Omit to
    /// open a blank, passive panel.
    #[arg(long, value_name = "TARGET")]
    connect: Option<String>,
}

/// Rasterise the faceplate SVG to an iced image handle (half native resolution — iced
/// scales it to the window anyway).
fn render_svg() -> Option<iced::widget::image::Handle> {
    const WIDTH: u32 = 1089;
    const HEIGHT: u32 = 741;
    const SCALE: f32 = 0.5;
    let tree = usvg::Tree::from_data(DELUGE_SVG, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(WIDTH, HEIGHT)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(SCALE, SCALE), &mut pixmap.as_mut());
    Some(iced::widget::image::Handle::from_rgba(WIDTH, HEIGHT, pixmap.data().to_vec()))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let cli = Cli::parse();

    let link = match cli.connect {
        Some(path) => {
            println!("deluge-simulator: connecting to brain at {path} ...");
            Some(PanelLink::connect(&path)?)
        }
        None => {
            println!("deluge-simulator: no --connect socket; opening a passive (blank) panel.");
            None
        }
    };

    let svg = render_svg();

    // iced's init closure must be `Fn` (called once); hand the link + svg through a
    // Mutex<Option<_>> so the first call can `take()` them.
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
    .run()?;
    Ok(())
}

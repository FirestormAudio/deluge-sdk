//! The simulator application shell (iced): render the panel from inbound
//! illumination frames, forward input as [`FromDeluge`]. The Deluge "brain" on the
//! other end of the [`PanelLink`] owns all UI logic — this is a faithful front panel,
//! not a reimplementation. (Trimmed from spark's `simulator_app`: the engine /
//! DelugeUI / settings / remote wiring is gone, replaced by the protocol link.)

use crate::display::SimulatorDisplay;
use crate::hardware::{HardwareButton, HardwareEncoder, HardwareLED};
use crate::hardware_state::DelugeHardware;
use crate::link::{self, LinkKind};
use crate::pad_grid::PadGrid;
use crate::renderer::DynamicElementsRenderer;
use crate::rgb::RGB;

use deluge_protocol::{FromDeluge, ToDeluge};

use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
use iced::{
    Color, Element, Length, Task, Theme,
    widget::{Canvas, Image, Stack, container, image},
};

/// Messages produced by the canvas (input) and the periodic tick.
#[derive(Debug, Clone)]
pub enum SimulatorMessage {
    /// 60 Hz tick: drain inbound illumination frames.
    Tick,
    PadPressed { col: usize, row: usize },
    PadReleased { col: usize, row: usize },
    ButtonPressed(HardwareButton),
    ButtonReleased(HardwareButton),
    EncoderRotated { encoder: HardwareEncoder, delta: i32 },
    EncoderPressed(HardwareEncoder),
    EncoderReleased(HardwareEncoder),
    ToggleStickyKeys,
}

pub struct DelugeSimulator {
    /// Persistent canvas renderer — owns display, pad grid, and hardware state.
    renderer: DynamicElementsRenderer,
    /// Pre-rasterised SVG faceplate (drawn behind the canvas).
    svg_background: Option<image::Handle>,
    /// How the panel is driven (protocol brain, in-process SDK app, or passive).
    link: LinkKind,
}

impl DelugeSimulator {
    pub fn new(link: LinkKind, svg_background: Option<image::Handle>) -> Self {
        Self {
            renderer: DynamicElementsRenderer::new(
                SimulatorDisplay::new(),
                PadGrid::new(),
                DelugeHardware::new(),
            ),
            svg_background,
            link,
        }
    }

    pub fn update(&mut self, message: SimulatorMessage) -> Task<SimulatorMessage> {
        match message {
            SimulatorMessage::Tick => self.drain_inbound(),

            SimulatorMessage::PadPressed { col, row } => {
                self.renderer.grid.set_pressed(col, row, true);
                self.renderer.pad_cache.clear();
                self.send(FromDeluge::PadPressed { col: col as u8, row: row as u8 });
            }
            SimulatorMessage::PadReleased { col, row } => {
                self.renderer.grid.set_pressed(col, row, false);
                self.renderer.pad_cache.clear();
                self.send(FromDeluge::PadReleased { col: col as u8, row: row as u8 });
            }

            SimulatorMessage::ButtonPressed(b) => {
                self.renderer.pressed_buttons.insert(b);
                self.renderer.controls_cache.clear();
                self.send(FromDeluge::ButtonPressed { id: link::button_to_id(b) });
            }
            SimulatorMessage::ButtonReleased(b) => {
                self.renderer.pressed_buttons.remove(&b);
                self.renderer.controls_cache.clear();
                self.send(FromDeluge::ButtonReleased { id: link::button_to_id(b) });
            }

            SimulatorMessage::EncoderRotated { encoder, delta } => {
                self.renderer.hardware.rotate_encoder(encoder, delta);
                self.renderer.controls_cache.clear();
                if let Some(id) = link::encoder_to_id(encoder) {
                    self.send(FromDeluge::EncoderRotated {
                        id,
                        delta: delta.clamp(-128, 127) as i8,
                    });
                }
            }
            SimulatorMessage::EncoderPressed(e) => {
                if let Some(id) = link::encoder_push_id(e) {
                    self.send(FromDeluge::ButtonPressed { id });
                }
            }
            SimulatorMessage::EncoderReleased(e) => {
                if let Some(id) = link::encoder_push_id(e) {
                    self.send(FromDeluge::ButtonReleased { id });
                }
            }

            SimulatorMessage::ToggleStickyKeys => {
                self.renderer.sticky_keys_enabled = !self.renderer.sticky_keys_enabled;
                self.renderer.controls_cache.clear();
            }
        }
        Task::none()
    }

    fn send(&self, msg: FromDeluge) {
        match &self.link {
            LinkKind::Protocol(link) => {
                let _ = link.outbound.send(msg);
            }
            LinkKind::InProcess(link) => link.send_input(msg),
            LinkKind::None => {}
        }
    }

    /// Drain inbound illumination and apply it to the panel.
    fn drain_inbound(&mut self) {
        match &self.link {
            LinkKind::Protocol(_) => self.drain_protocol(),
            LinkKind::InProcess(_) => self.apply_shared_panel(),
            LinkKind::None => {}
        }
    }

    /// Protocol link: decode framed `ToDeluge` messages and apply them.
    fn drain_protocol(&mut self) {
        // Collect first so we don't hold an immutable borrow of `self.link` while
        // applying (which borrows `self` mutably).
        let mut frames = Vec::new();
        if let LinkKind::Protocol(link) = &self.link {
            while let Ok(frame) = link.inbound.try_recv() {
                frames.push(frame);
            }
        }
        for (type_byte, data) in frames {
            if let Some(msg) = ToDeluge::decode(type_byte, &data) {
                self.apply(msg);
            }
        }
    }

    /// In-process link: mirror the shared panel into the renderer, repainting only
    /// the categories whose change-generation advanced since the last frame.
    fn apply_shared_panel(&mut self) {
        // Snapshot what changed, dropping the `&self.link` borrow before mutating.
        let LinkKind::InProcess(link) = &self.link else {
            return;
        };
        let panel = link.panel.clone();
        let (seen_d, seen_p, seen_c) = (link.seen_display, link.seen_pads, link.seen_controls);
        let (gen_d, gen_p, gen_c) = (panel.display_gen(), panel.pads_gen(), panel.controls_gen());

        let display = (gen_d != seen_d).then(|| panel.display_snapshot());
        let pads = (gen_p != seen_p).then(|| panel.pads_snapshot());
        let controls = (gen_c != seen_c)
            .then(|| (panel.leds_snapshot(), panel.knobs_snapshot(), panel.synced_led()));

        if let Some(buf) = display {
            self.update_display_from_buffer(&buf);
        }
        if let Some(grid) = pads {
            for (col, rows) in grid.iter().enumerate() {
                for (row, rgb) in rows.iter().enumerate() {
                    self.renderer.grid.set(col, row, RGB::new(rgb[0], rgb[1], rgb[2]));
                }
            }
            self.renderer.pad_cache.clear();
        }
        if let Some((leds, knobs, synced)) = controls {
            for (index, on) in leds.iter().enumerate() {
                if let Some(led) = link::led_index_to_led(index as u8) {
                    self.renderer.hardware.set_led_state(led, *on);
                }
            }
            self.renderer.hardware.set_led_state(HardwareLED::Synced, synced);
            self.set_knob_indicator(0, knobs[0]);
            self.set_knob_indicator(1, knobs[1]);
            self.renderer.controls_cache.clear();
        }

        // Record the generations we've now applied.
        if let LinkKind::InProcess(link) = &mut self.link {
            link.seen_display = gen_d;
            link.seen_pads = gen_p;
            link.seen_controls = gen_c;
        }
    }

    fn apply(&mut self, msg: ToDeluge) {
        match msg {
            ToDeluge::UpdateDisplay(buf) => self.update_display_from_buffer(buf),
            ToDeluge::ClearDisplay => {
                self.renderer.display.clear(BinaryColor::Off).unwrap();
                self.renderer.oled_cache.clear();
            }
            ToDeluge::SetPadRgb { col, row, rgb } => {
                self.renderer
                    .grid
                    .set(col as usize, row as usize, RGB::new(rgb[0], rgb[1], rgb[2]));
                self.renderer.pad_cache.clear();
            }
            ToDeluge::SetAllPads(buf) => {
                for col in 0..18usize {
                    for row in 0..8usize {
                        let o = (col * 8 + row) * 3;
                        self.renderer
                            .grid
                            .set(col, row, RGB::new(buf[o], buf[o + 1], buf[o + 2]));
                    }
                }
                self.renderer.pad_cache.clear();
            }
            ToDeluge::ClearAllPads => {
                self.renderer.grid.clear();
                self.renderer.pad_cache.clear();
            }
            ToDeluge::SetLed { index, on } => {
                if let Some(led) = link::led_index_to_led(index) {
                    self.renderer.hardware.set_led_state(led, on);
                    self.renderer.controls_cache.clear();
                }
            }
            ToDeluge::SetSyncedLed(on) => {
                self.renderer.hardware.set_led_state(HardwareLED::Synced, on);
                self.renderer.controls_cache.clear();
            }
            ToDeluge::SetKnobIndicator { which, levels } => self.set_knob_indicator(which, levels),
            ToDeluge::ClearAllLeds => {
                for led in HardwareLED::all_leds() {
                    self.renderer.hardware.set_led_state(led, false);
                }
                self.renderer.controls_cache.clear();
            }
            // No panel representation (audio/CV/gate/brightness/handshake).
            ToDeluge::SetCv { .. }
            | ToDeluge::SetGate { .. }
            | ToDeluge::SetBrightness(_)
            | ToDeluge::GetVersion
            | ToDeluge::Ping => {}
        }
    }

    /// Light the 4 segment LEDs of a gold knob ring from inbound levels (`which`:
    /// 0 = lower, 1 = upper). A segment is on when its level is non-zero.
    fn set_knob_indicator(&mut self, which: u8, levels: [u8; 4]) {
        use HardwareLED::*;
        let segs = if which == 0 {
            [LowerGoldIndicator1, LowerGoldIndicator2, LowerGoldIndicator3, LowerGoldIndicator4]
        } else {
            [UpperGoldIndicator1, UpperGoldIndicator2, UpperGoldIndicator3, UpperGoldIndicator4]
        };
        for (i, led) in segs.into_iter().enumerate() {
            self.renderer.hardware.set_led_state(led, levels[i] > 0);
        }
        self.renderer.controls_cache.clear();
    }

    /// Unpack a 768-byte SSD1309 page-major framebuffer into the canvas display.
    /// `buf[page*128 + col]`, bit `b` = panel row `page*8 + b`; the 43 visible rows
    /// start at panel row 5 (the bezel rows above are skipped).
    fn update_display_from_buffer(&mut self, buffer: &[u8]) {
        const HARDWARE_TOPMOST: usize = 5;
        const OLED_HEIGHT: usize = 43;
        if buffer.len() != 768 {
            return;
        }
        self.renderer.display.clear(BinaryColor::Off).unwrap();
        for page in 0..6usize {
            for col in 0..128usize {
                let byte = buffer[page * 128 + col];
                for bit in 0..8usize {
                    if byte & (1 << bit) != 0 {
                        let panel_row = page * 8 + bit;
                        if panel_row < HARDWARE_TOPMOST {
                            continue;
                        }
                        let ui_row = panel_row - HARDWARE_TOPMOST;
                        if ui_row < OLED_HEIGHT {
                            self.renderer.display.set_pixel(col, ui_row, true);
                        }
                    }
                }
            }
        }
        self.renderer.oled_cache.clear();
    }

    pub fn view(&self) -> Element<'_, SimulatorMessage> {
        let canvas = Canvas::new(&self.renderer).width(Length::Fill).height(Length::Fill);
        let content = if let Some(ref svg_handle) = self.svg_background {
            let background: Image = image(svg_handle.clone())
                .content_fit(iced::ContentFit::Contain)
                .filter_method(image::FilterMethod::Linear)
                .width(Length::Fill)
                .height(Length::Fill);
            container(
                Stack::new()
                    .push(background)
                    .push(canvas)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
        } else {
            container(canvas)
        };
        content
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(Color::from_rgb(0.0, 0.0, 0.0).into()),
                border: iced::Border {
                    color: Color::from_rgb(0.2, 0.2, 0.2),
                    width: 4.0,
                    radius: 8.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    pub fn title(&self) -> String {
        String::from("Deluge Emulator")
    }

    pub fn subscription(&self) -> iced::Subscription<SimulatorMessage> {
        iced::time::every(std::time::Duration::from_millis(16)).map(|_| SimulatorMessage::Tick)
    }
}

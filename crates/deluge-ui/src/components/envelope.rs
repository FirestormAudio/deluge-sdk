//! ADSR Envelope drawing utilities
//!
//! Provides functions to draw ADSR (Attack, Decay, Sustain, Release) envelopes
//! with visual indicators, replicating the Deluge firmware's envelope visualization.

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, Primitive, PrimitiveStyle, Rectangle},
};

use crate::DottedLine;
// Required for no_std ARM target where core_float_math is unavailable.
#[allow(unused_imports)]
use crate::prelude::F32Ext as _;

/// ADSR envelope parameters (all values 0.0-1.0)
#[derive(Debug, Clone, Copy)]
pub struct ADSR {
    /// Attack time (0.0 = instant, 1.0 = maximum)
    pub attack: f32,
    /// Decay time (0.0 = instant, 1.0 = maximum)
    pub decay: f32,
    /// Sustain level (0.0 = silent, 1.0 = full level)
    pub sustain: f32,
    /// Release time (0.0 = instant, 1.0 = maximum)
    pub release: f32,
}

impl Default for ADSR {
    fn default() -> Self {
        Self {
            attack: 0.0,
            decay: 0.2,
            sustain: 0.7,
            release: 0.3,
        }
    }
}

/// Which ADSR stage is currently selected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
pub struct Envelope {
    adsr: ADSR,
    selected_stage: Option<EnvelopeStage>,
    position: Point,
    size: Size,
}

impl Envelope {
    pub fn new(adsr: ADSR, position: Point, size: Size) -> Self {
        Self {
            adsr,
            selected_stage: None,
            position,
            size,
        }
    }

    pub fn with_selected_stage(mut self, stage: Option<EnvelopeStage>) -> Self {
        self.selected_stage = stage;
        self
    }
}

/// Draw an ADSR envelope visualization
///
/// # Arguments
/// * `display` - The display to draw on
/// * `x` - X coordinate of the envelope area (left edge)
/// * `y` - Y coordinate of the envelope area (top edge)
/// * `width` - Width of the envelope drawing area
/// * `height` - Height of the envelope drawing area
/// * `params` - ADSR parameters (all 0.0-1.0)
/// * `selected_stage` - Optional stage to highlight with selection indicator
///
/// # Example
/// ```no_run
/// # use deluge_ui_toolkit::{ADSR, Envelope, EnvelopeStage};
/// # use embedded_graphics::prelude::*;
/// # use embedded_graphics_simulator::SimulatorDisplay;
/// # use embedded_graphics::pixelcolor::BinaryColor;
/// # let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
/// let adsr = ADSR { attack: 0.3, decay: 0.4, sustain: 0.6, release: 0.5 };
/// let envelope = Envelope::new(adsr, Point::new(4, 20), Size::new(120, 30))
///     .with_selected_stage(Some(EnvelopeStage::Attack));
/// envelope.draw(&mut display).ok();
/// ```
impl Drawable for Envelope {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        // Clamp parameters to 0.0-1.0
        let attack = self.adsr.attack.clamp(0.0, 1.0);
        let decay = self.adsr.decay.clamp(0.0, 1.0);
        let sustain = self.adsr.sustain.clamp(0.0, 1.0);
        let release = self.adsr.release.clamp(0.0, 1.0);

        // Constants
        let start_x = self.position.x;
        let start_y = self.position.y;
        let end_y = self.position.y + self.size.height as i32;
        let draw_width = self.size.width as i32;
        let draw_height = self.size.height as i32;
        let max_segment_width = draw_width as f32 / 4.0;

        // Calculate widths for each segment
        let attack_width = attack * max_segment_width;

        // Apply sigmoid-like curve to decay for visual effect (steep start, gradual end)
        let decay_normalized = sigmoid_like_curve(decay, 1.0, 10.0);
        let decay_width = decay_normalized * max_segment_width;

        // Calculate X positions for stage transitions
        let attack_x = (start_x as f32 + attack_width).round() as i32;
        let decay_x = (attack_x as f32 + decay_width).round() as i32;
        let sustain_x = start_x + (max_segment_width * 3.0) as i32; // Fixed position
        let release_x =
            (sustain_x as f32 + release * (start_x + draw_width - sustain_x) as f32).round() as i32;

        // Calculate Y positions
        let base_y = start_y + draw_height;
        let peak_y = start_y; // Top of attack
        let sustain_y = (base_y as f32 - sustain * draw_height as f32).round() as i32;

        let points = [
            Point::new(start_x, base_y),              // Start
            Point::new(attack_x, peak_y),             // Attack peak
            Point::new(decay_x, sustain_y),           // Decay to sustain
            Point::new(sustain_x, sustain_y),         // Sustain hold
            Point::new(release_x, base_y),            // Release to base
            Point::new(start_x + draw_width, base_y), // End
        ];

        // Draw envelope lines
        for point_pair in points.windows(2) {
            if let [start, end] = point_pair {
                Line::new(*start, *end)
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                    .draw(display)?;
            }
        }

        // Draw vertical dotted lines at stage transitions
        DottedLine::new(
            Point::new(attack_x, start_y - 2),
            Point::new(attack_x, end_y),
            4,
        )
        .draw(display)?;
        DottedLine::new(
            Point::new(decay_x, start_y - 2),
            Point::new(decay_x, end_y),
            4,
        )
        .draw(display)?;

        // Only draw sustain vertical line if not at peak or if line is visible
        if sustain_y > start_y {
            DottedLine::new(
                Point::new(sustain_x, start_y - 2),
                Point::new(sustain_x, end_y),
                4,
            )
            .draw(display)?;
        } else {
            DottedLine::new(
                Point::new(sustain_x, sustain_y),
                Point::new(sustain_x, end_y),
                4,
            )
            .draw(display)?;
        }

        // Draw stage transition indicators
        let mut drawn_positions: [(i32, i32); 4] = [(-1, -1); 4];
        let mut drawn_count = 0;

        draw_transition_indicator(
            display,
            attack_x,
            peak_y,
            self.selected_stage == Some(EnvelopeStage::Attack),
            &mut drawn_positions,
            &mut drawn_count,
        )?;

        draw_transition_indicator(
            display,
            decay_x,
            sustain_y,
            self.selected_stage == Some(EnvelopeStage::Decay),
            &mut drawn_positions,
            &mut drawn_count,
        )?;

        // Sustain indicator in middle of sustain segment
        let sustain_indicator_x = decay_x + (sustain_x - decay_x) / 2;
        draw_transition_indicator(
            display,
            sustain_indicator_x,
            sustain_y,
            self.selected_stage == Some(EnvelopeStage::Sustain),
            &mut drawn_positions,
            &mut drawn_count,
        )?;

        draw_transition_indicator(
            display,
            release_x,
            base_y,
            self.selected_stage == Some(EnvelopeStage::Release),
            &mut drawn_positions,
            &mut drawn_count,
        )?;

        Ok(())
    }
}

/// Sigmoid-like curve function for visual smoothing
/// Maps input from 0-max to 0-1 with configurable steepness
fn sigmoid_like_curve(value: f32, max: f32, steepness: f32) -> f32 {
    let normalized = value / max;
    let x = (normalized - 0.5) * steepness;
    1.0 / (1.0 + (-x).exp())
}

/// Draw a transition indicator (square) at a stage transition point
fn draw_transition_indicator<D>(
    display: &mut D,
    center_x: i32,
    center_y: i32,
    is_selected: bool,
    drawn_positions: &mut [(i32, i32); 4],
    drawn_count: &mut usize,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    // Check for overlaps with previously drawn indicators
    if !is_selected {
        for &(prev_x, prev_y) in drawn_positions.iter().take(*drawn_count) {
            if center_x == prev_x && center_y == prev_y {
                // Overlap detected, skip drawing
                return Ok(());
            }
        }
    }

    const SQUARE_SIZE: i32 = 2;
    const INNER_SIZE: i32 = SQUARE_SIZE - 1;

    // Clear the inner region
    for dx in -INNER_SIZE..=INNER_SIZE {
        for dy in -INNER_SIZE..=INNER_SIZE {
            Pixel(Point::new(center_x + dx, center_y + dy), BinaryColor::Off).draw(display)?;
        }
    }

    // If selected, invert the inner area for highlight
    if is_selected {
        // For simplicity, we'll draw a filled rectangle for selected state
        Rectangle::new(
            Point::new(center_x - INNER_SIZE, center_y - INNER_SIZE),
            Size::new((INNER_SIZE * 2 + 1) as u32, (INNER_SIZE * 2 + 1) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;

        // Record this position
        if *drawn_count < 4 {
            drawn_positions[*drawn_count] = (center_x, center_y);
            *drawn_count += 1;
        }
    }

    // Draw the square outline
    Rectangle::new(
        Point::new(center_x - SQUARE_SIZE, center_y - SQUARE_SIZE),
        Size::new((SQUARE_SIZE * 2 + 1) as u32, (SQUARE_SIZE * 2 + 1) as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_draw_envelope_default() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        Envelope::new(ADSR::default(), Point::new(4, 20), Size::new(120, 30))
            .draw(&mut display)
            .unwrap();
    }

    #[test]
    fn test_draw_envelope_with_selection() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));

        let params = ADSR {
            attack: 0.3,
            decay: 0.4,
            sustain: 0.6,
            release: 0.5,
        };

        Envelope::new(params, Point::new(4, 20), Size::new(120, 30))
            .with_selected_stage(Some(EnvelopeStage::Attack))
            .draw(&mut display)
            .unwrap();
    }

    #[test]
    fn test_sigmoid_curve() {
        assert!((sigmoid_like_curve(0.0, 1.0, 10.0) - 0.0).abs() < 0.1);
        assert!((sigmoid_like_curve(0.5, 1.0, 10.0) - 0.5).abs() < 0.1);
        assert!((sigmoid_like_curve(1.0, 1.0, 10.0) - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_envelope_params_clamping() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));

        // Test with out-of-range values
        let params = ADSR {
            attack: 1.5,
            decay: -0.2,
            sustain: 2.0,
            release: -1.0,
        };
        Envelope::new(params, Point::new(4, 20), Size::new(120, 30))
            .draw(&mut display)
            .unwrap();
    }
}

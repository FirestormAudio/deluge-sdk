//! Waveform display widget for the RGB LED grid.
//!
//! A stateless component that renders a sample slice to fit the available grid
//! area. Zoom/scroll is handled externally. Based on the Deluge firmware's
//! `WaveformRenderer`.

use crate::color::ColorExt as _;
use crate::component::{Component, FlexibleComponent, Size};
#[allow(unused_imports)] // needed on targets whose `core` lacks inherent f32 math
use crate::float_ext::F32Ext as _;
use crate::Grid;
use alloc::vec::Vec;
use deluge_bsp::rgb::Color as RGB;

#[cfg(all(feature = "simd", target_arch = "aarch64"))]
use core::arch::aarch64::*;
#[cfg(all(feature = "simd", target_arch = "arm"))]
use core::arch::arm::*;

/// Reduce a contiguous f32 slice to `(min, max)` using NEON.
///
/// # Safety
/// `samples` must have at least 4 elements and the `neon` target feature must be
/// enabled.
#[cfg(all(feature = "simd", any(target_arch = "aarch64", target_arch = "arm")))]
#[target_feature(enable = "neon")]
unsafe fn find_min_max_f32_neon(samples: &[f32]) -> (f32, f32) {
    unsafe {
        let ptr = samples.as_ptr();
        let chunks = samples.len() / 4;

        let mut min_acc = vld1q_f32(ptr);
        let mut max_acc = vld1q_f32(ptr);
        for i in 1..chunks {
            let v = vld1q_f32(ptr.add(i * 4));
            min_acc = vminq_f32(min_acc, v);
            max_acc = vmaxq_f32(max_acc, v);
        }

        #[cfg(target_arch = "aarch64")]
        {
            let mut min_val = vminvq_f32(min_acc);
            let mut max_val = vmaxvq_f32(max_acc);
            for &s in &samples[chunks * 4..] {
                min_val = min_val.min(s);
                max_val = max_val.max(s);
            }
            (min_val, max_val)
        }

        #[cfg(target_arch = "arm")]
        {
            let min_p = vpmin_f32(vget_low_f32(min_acc), vget_high_f32(min_acc));
            let mut min_val = vget_lane_f32(vpmin_f32(min_p, min_p), 0);
            let max_p = vpmax_f32(vget_low_f32(max_acc), vget_high_f32(max_acc));
            let mut max_val = vget_lane_f32(vpmax_f32(max_p, max_p), 0);
            for &s in &samples[chunks * 4..] {
                min_val = min_val.min(s);
                max_val = max_val.max(s);
            }
            (min_val, max_val)
        }
    }
}

const SAMPLES_TO_READ_PER_COL_MAGNITUDE: u32 = 9;
const MIN_WAVEFORM_HEIGHT: f32 = 0.1;

/// Peak data for a single column.
#[derive(Debug, Clone, Copy)]
struct ColumnPeaks {
    min: f32,
    max: f32,
}

impl ColumnPeaks {
    fn with_min_height(self, min_height: f32) -> Self {
        if self.max - self.min < min_height {
            let mid = (self.min + self.max) / 2.0;
            let half_height = min_height / 2.0;
            Self {
                min: mid - half_height,
                max: mid + half_height,
            }
        } else {
            self
        }
    }

    fn to_row_range(self, half_rows: f32) -> (i32, i32) {
        let y_top = -self.max * half_rows;
        let y_bottom = -self.min * half_rows;

        let y_start = y_top.floor() as i32;
        let y_end = y_bottom.ceil() as i32;

        let y_start = y_start.max(-(half_rows as i32));
        let y_end = y_end.min(half_rows as i32);

        (y_start, y_end)
    }
}

/// Waveform display component.
pub struct WaveformDisplayComponent {
    size: Size,
    sample_data: Vec<f32>,
    color: RGB,
}

impl WaveformDisplayComponent {
    pub fn new(size: Size, data: &[f32]) -> Self {
        Self {
            size,
            sample_data: data.to_vec(),
            color: RGB::cyan(),
        }
    }

    /// The current sample data.
    pub fn sample_data(&self) -> &[f32] {
        &self.sample_data
    }

    /// Set the sample data to display (rendered to fit the available area).
    pub fn set_sample_data(&mut self, data: &[f32]) {
        self.sample_data = data.to_vec();
    }

    /// Set the waveform colour.
    pub fn set_color(&mut self, color: RGB) {
        self.color = color;
    }

    fn find_peaks_per_col(&self) -> Vec<Option<ColumnPeaks>> {
        let num_cols = self.size.cols;
        let num_samples = self.sample_data.len();

        if num_samples == 0 {
            return alloc::vec![None; num_cols];
        }

        let samples_per_col = num_samples as f64 / num_cols as f64;

        (0..num_cols)
            .map(|col| {
                let start_idx = (col as f64 * samples_per_col) as usize;
                let end_idx = ((col + 1) as f64 * samples_per_col).min(num_samples as f64) as usize;

                if start_idx >= end_idx || start_idx >= num_samples {
                    return None;
                }

                let chunk = &self.sample_data[start_idx..end_idx];

                let max_samples_to_read = 1 << SAMPLES_TO_READ_PER_COL_MAGNITUDE;
                let step = (chunk.len() / max_samples_to_read).max(1);

                #[cfg(all(feature = "simd", any(target_arch = "aarch64", target_arch = "arm")))]
                if step == 1 && chunk.len() >= 4 {
                    let (min, max) = unsafe { find_min_max_f32_neon(chunk) };
                    return Some(ColumnPeaks {
                        min: min.clamp(-1.0, 1.0),
                        max: max.clamp(-1.0, 1.0),
                    });
                }

                chunk
                    .iter()
                    .step_by(step)
                    .copied()
                    .fold(None::<(f32, f32)>, |acc, sample| {
                        Some(match acc {
                            None => (sample, sample),
                            Some((min, max)) => (min.min(sample), max.max(sample)),
                        })
                    })
                    .map(|(min, max)| ColumnPeaks {
                        min: min.clamp(-1.0, 1.0),
                        max: max.clamp(-1.0, 1.0),
                    })
            })
            .collect()
    }

    fn calculate_edge_coverage(
        y: i32,
        y_start: i32,
        y_stop: i32,
        peaks: &ColumnPeaks,
        half_rows: f32,
    ) -> f32 {
        if y == y_start {
            let edge_pos = -peaks.max * half_rows;
            (edge_pos - edge_pos.floor()).max(0.5)
        } else if y == y_stop {
            let edge_pos = -peaks.min * half_rows;
            (edge_pos.ceil() - edge_pos).max(0.5)
        } else {
            1.0
        }
    }

    fn apply_color_with_brightness(&self, brightness: u8, coverage: f32) -> RGB {
        let color_amount = (brightness as f32 * coverage.clamp(0.0, 1.0)) as u32;
        let value = (color_amount * color_amount) >> 8;
        RGB::new(
            ((value * self.color.r as u32) >> 8) as u8,
            ((value * self.color.g as u32) >> 8) as u8,
            ((value * self.color.b as u32) >> 8) as u8,
        )
    }

    fn draw_col_bar(&self, grid: &mut Grid, col: usize, peaks: &ColumnPeaks, brightness: u8) {
        let half_rows = self.size.rows as f32 / 2.0;
        let adjusted_peaks = peaks.with_min_height(MIN_WAVEFORM_HEIGHT);
        let (y_start, y_stop) = adjusted_peaks.to_row_range(half_rows);

        (y_start..=y_stop).for_each(|y| {
            let coverage =
                Self::calculate_edge_coverage(y, y_start, y_stop, &adjusted_peaks, half_rows);
            let final_color = self.apply_color_with_brightness(brightness, coverage);
            let grid_row = (half_rows as i32 + y).clamp(0, self.size.rows as i32 - 1) as usize;
            grid.set_pad(grid_row, col, final_color);
        });
    }
}

impl Component for WaveformDisplayComponent {
    fn render(&self) -> Grid {
        let mut grid = Grid::new();
        if self.sample_data.is_empty() {
            return grid;
        }

        let peaks_data = self.find_peaks_per_col();
        peaks_data
            .iter()
            .enumerate()
            .filter_map(|(col, peaks)| peaks.map(|p| (col, p)))
            .for_each(|(col, peaks)| {
                self.draw_col_bar(&mut grid, col, &peaks, 200);
            });

        grid
    }

    fn needs_redraw(&self) -> bool {
        !self.sample_data.is_empty()
    }

    fn get_size(&self) -> Size {
        self.size
    }
}

impl FlexibleComponent for WaveformDisplayComponent {
    fn set_size(&mut self, size: Size) {
        self.size = size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_renders_blank() {
        let comp = WaveformDisplayComponent::new(Size::new(8, 16), &[]);
        let grid = comp.render();
        for row in 0..8 {
            for col in 0..16 {
                assert_eq!(grid.get_pad(row, col), RGB::BLACK);
            }
        }
    }

    #[test]
    fn nonempty_data_lights_pads() {
        let data: Vec<f32> = (0..256).map(|i| ((i as f32) / 128.0 - 1.0)).collect();
        let comp = WaveformDisplayComponent::new(Size::new(8, 16), &data);
        let grid = comp.render();
        let lit = (0..8)
            .flat_map(|r| (0..16).map(move |c| (r, c)))
            .filter(|&(r, c)| grid.get_pad(r, c) != RGB::BLACK)
            .count();
        assert!(lit > 0);
    }
}

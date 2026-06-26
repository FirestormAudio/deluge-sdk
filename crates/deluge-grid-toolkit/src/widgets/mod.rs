//! Stateless render widgets — each takes plain colours/values and renders a
//! [`crate::Grid`].

pub mod automation_column;
pub mod clip_cell;
pub mod text_keyboard;
pub mod waveform_display;

pub use automation_column::{
    AutomationColor, BipolarAutomationColumn, NoteVelocityColumn, UnipolarAutomationColumn,
    bipolar_pad_value, unipolar_pad_value,
};
pub use clip_cell::{CellPlaybackState, ClipCellComponent};
pub use text_keyboard::{KeyPress, KeyboardLayout, TextKeyboardComponent};
pub use waveform_display::draw_waveform;

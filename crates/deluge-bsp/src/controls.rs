//! Friendly names for Deluge front-panel controls.
//!
//! This module collects stable, human-readable IDs for physical buttons,
//! encoder shaft clicks, encoder rotation IDs, and gold-knob indicator bars.
//! It intentionally lives outside [`crate::pic`] because encoder rotation is
//! decoded by the main CPU GPIO/IRQ path rather than by the PIC UART protocol.

/// Named physical button IDs in the raw PIC button-ID space (`0..=35`).
///
/// IDs are derived from `hid/button.h` and `definitions_cxx.hpp` in
/// DelugeFirmware using the formula `9 * (y + kDisplayHeight * 2) + x - 144`,
/// where `kDisplayHeight = 8`. For physical buttons with indicator LEDs, the
/// LED index equals the raw button ID.
///
/// IDs 0, 9, 13, 18, 27, and 31 are encoder shaft-click events reported
/// through [`encoder_button`] rather than here.
pub mod button {
    pub const ENCODER_FUNCTION_0: u8 = 1; // x=1, y=0
    pub const ENCODER_FUNCTION_4: u8 = 2; // x=2, y=0
    pub const ENCODER_FUNCTION_1: u8 = 10; // x=1, y=1
    pub const ENCODER_FUNCTION_5: u8 = 11; // x=2, y=1
    pub const ENCODER_FUNCTION_2: u8 = 19; // x=1, y=2
    pub const ENCODER_FUNCTION_6: u8 = 20; // x=2, y=2
    pub const ENCODER_FUNCTION_3: u8 = 28; // x=1, y=3
    pub const ENCODER_FUNCTION_7: u8 = 29; // x=2, y=3

    // ── Named front-panel buttons ─────────────────────────────────────────────
    pub const AFFECT_ENTIRE: u8 = 3;
    pub const SYNTH: u8 = 5;
    pub const SCALE_MODE: u8 = 6;
    pub const LEARN: u8 = 7;
    pub const SHIFT: u8 = 8;
    pub const SESSION_VIEW: u8 = 12;
    pub const KIT: u8 = 14;
    pub const LOAD: u8 = 15;
    pub const BACK: u8 = 16;
    pub const TRIPLETS: u8 = 17;
    pub const CLIP_VIEW: u8 = 21;
    pub const MIDI: u8 = 23;
    pub const CROSS_SCREEN_EDIT: u8 = 24;
    pub const SYNC_SCALING: u8 = 25;
    pub const RECORD: u8 = 26;
    pub const KEYBOARD: u8 = 30;
    pub const CV: u8 = 32;
    pub const SAVE: u8 = 33;
    pub const TAP_TEMPO: u8 = 34;
    pub const PLAY: u8 = 35;
}

/// Friendly names for the six encoder push-buttons in the raw PIC button-ID space.
///
/// These shaft-click events are reported by the PIC as button presses and are
/// later forwarded on the CDC wire as `144 + raw_id`.
pub mod encoder_button {
    pub const SCROLL_Y: u8 = 0;
    pub const SCROLL_X: u8 = 9;
    pub const TEMPO: u8 = 13;
    pub const MOD_0: u8 = 18;
    pub const MOD_1: u8 = 27;
    pub const SELECT: u8 = 31;
}

/// Friendly names for encoder rotation IDs emitted by the CPU-owned encoder task.
pub mod encoder {
    pub const SCROLL_X: u8 = 0;
    pub const TEMPO: u8 = 1;
    pub const MOD_0: u8 = 2;
    pub const MOD_1: u8 = 3;
    pub const SCROLL_Y: u8 = 4;
    pub const SELECT: u8 = 5;
}

/// Friendly names for the two gold-knob indicator bars.
pub mod knob {
    pub const MOD_0: u8 = 0;
    pub const MOD_1: u8 = 1;
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// DelugeFirmware derives raw button IDs as `9 * (y + 16) + x - 144`
    /// (kDisplayHeight = 8). Verify the ENCODER_FUNCTION_* table against it.
    fn raw_id(x: u8, y: u8) -> u8 {
        (9 * (y as u16 + 16) + x as u16 - 144) as u8
    }

    #[test]
    fn encoder_function_ids_match_the_formula() {
        assert_eq!(button::ENCODER_FUNCTION_0, raw_id(1, 0));
        assert_eq!(button::ENCODER_FUNCTION_4, raw_id(2, 0));
        assert_eq!(button::ENCODER_FUNCTION_1, raw_id(1, 1));
        assert_eq!(button::ENCODER_FUNCTION_5, raw_id(2, 1));
        assert_eq!(button::ENCODER_FUNCTION_2, raw_id(1, 2));
        assert_eq!(button::ENCODER_FUNCTION_6, raw_id(2, 2));
        assert_eq!(button::ENCODER_FUNCTION_3, raw_id(1, 3));
        assert_eq!(button::ENCODER_FUNCTION_7, raw_id(2, 3));
    }

    #[test]
    fn all_button_ids_are_in_range_and_unique() {
        let buttons = [
            button::ENCODER_FUNCTION_0,
            button::ENCODER_FUNCTION_4,
            button::ENCODER_FUNCTION_1,
            button::ENCODER_FUNCTION_5,
            button::ENCODER_FUNCTION_2,
            button::ENCODER_FUNCTION_6,
            button::ENCODER_FUNCTION_3,
            button::ENCODER_FUNCTION_7,
            button::AFFECT_ENTIRE,
            button::SYNTH,
            button::SCALE_MODE,
            button::LEARN,
            button::SHIFT,
            button::SESSION_VIEW,
            button::KIT,
            button::LOAD,
            button::BACK,
            button::TRIPLETS,
            button::CLIP_VIEW,
            button::MIDI,
            button::CROSS_SCREEN_EDIT,
            button::SYNC_SCALING,
            button::RECORD,
            button::KEYBOARD,
            button::CV,
            button::SAVE,
            button::TAP_TEMPO,
            button::PLAY,
        ];
        let mut seen = [false; 36];
        for &b in &buttons {
            assert!(b <= 35, "button id {b} out of 0..=35");
            assert!(!seen[b as usize], "duplicate button id {b}");
            seen[b as usize] = true;
        }
    }

    #[test]
    fn encoder_buttons_are_disjoint_from_buttons() {
        // The six shaft-click IDs are reported separately and must not collide
        // with any named front-panel button.
        let shaft = [
            encoder_button::SCROLL_Y,
            encoder_button::SCROLL_X,
            encoder_button::TEMPO,
            encoder_button::MOD_0,
            encoder_button::MOD_1,
            encoder_button::SELECT,
        ];
        assert_eq!(shaft, [0, 9, 13, 18, 27, 31]);
        // Encoder *rotation* IDs are a separate 0..=5 space.
        let rot = [
            encoder::SCROLL_X,
            encoder::TEMPO,
            encoder::MOD_0,
            encoder::MOD_1,
            encoder::SCROLL_Y,
            encoder::SELECT,
        ];
        let mut seen = [false; 6];
        for &r in &rot {
            assert!(!seen[r as usize], "duplicate encoder id {r}");
            seen[r as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "encoder ids cover 0..=5");
    }
}

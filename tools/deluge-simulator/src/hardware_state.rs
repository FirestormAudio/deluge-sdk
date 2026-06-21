//! Hardware state tracking for the Deluge simulator
//!
//! This module manages the current state of all Deluge hardware controls.

use crate::hardware::{HardwareButton, HardwareEncoder, HardwareLED};
use std::collections::HashMap;

/// State of all Deluge hardware controls
#[derive(Clone)]
pub struct DelugeHardware {
    /// Current button states (true = pressed)
    button_states: HashMap<HardwareButton, bool>,

    /// Current encoder values
    encoder_values: HashMap<HardwareEncoder, i32>,

    /// Current LED states (true = on)
    led_states: HashMap<HardwareLED, bool>,
}

impl DelugeHardware {
    /// Create new hardware state with all controls in default positions
    pub fn new() -> Self {
        let mut button_states = std::collections::HashMap::new();
        for button in HardwareButton::all_buttons() {
            button_states.insert(button, false);
        }

        let mut encoder_values = std::collections::HashMap::new();
        for encoder in HardwareEncoder::all_encoders() {
            encoder_values.insert(encoder, 0);
        }

        let mut led_states = std::collections::HashMap::new();
        for led in HardwareLED::all_leds() {
            led_states.insert(led, false);
        }

        Self {
            button_states,
            encoder_values,
            led_states,
        }
    }

    /// Check if a button is currently pressed
    pub fn is_button_pressed(&self, button: HardwareButton) -> bool {
        self.button_states.get(&button).copied().unwrap_or(false)
    }

    /// Set button state
    pub fn set_button_state(&mut self, button: HardwareButton, pressed: bool) {
        self.button_states.insert(button, pressed);
    }

    /// Get encoder value
    pub fn get_encoder_value(&self, encoder: HardwareEncoder) -> i32 {
        self.encoder_values.get(&encoder).copied().unwrap_or(0)
    }

    /// Set encoder value
    pub fn set_encoder_value(&mut self, encoder: HardwareEncoder, value: i32) {
        self.encoder_values.insert(encoder, value);
    }

    /// Rotate encoder (increment or decrement)
    pub fn rotate_encoder(&mut self, encoder: HardwareEncoder, delta: i32) {
        let current = self.get_encoder_value(encoder);
        self.set_encoder_value(encoder, current + delta);
    }

    /// Check if an LED is currently on
    pub fn is_led_on(&self, led: HardwareLED) -> bool {
        self.led_states.get(&led).copied().unwrap_or(false)
    }

    /// Set LED state
    pub fn set_led_state(&mut self, led: HardwareLED, on: bool) {
        self.led_states.insert(led, on);
    }

    /// Toggle LED state
    pub fn toggle_led(&mut self, led: HardwareLED) {
        let current = self.is_led_on(led);
        self.set_led_state(led, !current);
    }

    /// Reset all controls to default state
    pub fn reset(&mut self) {
        for button in HardwareButton::all_buttons() {
            self.button_states.insert(button, false);
        }
        for encoder in HardwareEncoder::all_encoders() {
            self.encoder_values.insert(encoder, 0);
        }
        for led in HardwareLED::all_leds() {
            self.led_states.insert(led, false);
        }
    }
}

impl Default for DelugeHardware {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_creation() {
        let hardware = DelugeHardware::new();
        // All buttons should be unpressed
        assert!(!hardware.is_button_pressed(HardwareButton::Play));
        // All encoders should be at 0
        assert_eq!(hardware.get_encoder_value(HardwareEncoder::Select), 0);
    }

    #[test]
    fn test_button_state() {
        let mut hardware = DelugeHardware::new();
        hardware.set_button_state(HardwareButton::Play, true);
        assert!(hardware.is_button_pressed(HardwareButton::Play));
    }

    #[test]
    fn test_encoder_rotation() {
        let mut hardware = DelugeHardware::new();
        hardware.rotate_encoder(HardwareEncoder::Select, 5);
        assert_eq!(hardware.get_encoder_value(HardwareEncoder::Select), 5);
        hardware.rotate_encoder(HardwareEncoder::Select, -3);
        assert_eq!(hardware.get_encoder_value(HardwareEncoder::Select), 2);
    }
}

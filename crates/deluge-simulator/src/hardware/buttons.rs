use crate::hardware::{HardwareLED, leds::LEDState};

/// Hardware buttons on the Deluge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwareButton {
    // Encoder select buttons (8 small buttons above encoders)
    EncoderFunction1,
    EncoderFunction2,
    EncoderFunction3,
    EncoderFunction4,
    EncoderFunction5,
    EncoderFunction6,
    EncoderFunction7,
    EncoderFunction8,

    // Top center buttons (around display)
    Back,
    Load,
    Save,
    Copy,

    // Top right buttons
    Fill,
    Select,
    TapTempo,

    // Far right buttons
    Play,
    Record,
    Shift,

    // Under-screen row buttons
    Time,
    Quantize,
    Automation,
    Transform,

    // Mode buttons
    Session,
    Clip,

    // Misc buttons (below mode buttons)
    Scope,
    Keyboard,
    Scale,
    Loop,
}

impl HardwareButton {
    /// Get the display name for this button
    pub fn name(&self) -> &'static str {
        match self {
            Self::EncoderFunction1 => "Select 1",
            Self::EncoderFunction2 => "Select 2",
            Self::EncoderFunction3 => "Select 3",
            Self::EncoderFunction4 => "Select 4",
            Self::EncoderFunction5 => "Select 5",
            Self::EncoderFunction6 => "Select 6",
            Self::EncoderFunction7 => "Select 7",
            Self::EncoderFunction8 => "Select 8",
            Self::Back => "BACK",
            Self::Load => "LOAD",
            Self::Save => "SAVE",
            Self::Copy => "COPY",
            Self::Fill => "FILL",
            Self::Select => "SELECT",
            Self::TapTempo => "TAP TEMPO",
            Self::Play => "▶ PLAY",
            Self::Record => "● RECORD",
            Self::Shift => "SHIFT",
            Self::Scope => "SCOPE",
            Self::Session => "SESSION",
            Self::Keyboard => "KEYBOARD",
            Self::Time => "TIME",
            Self::Quantize => "QUANTIZE",
            Self::Automation => "AUTOMATION",
            Self::Transform => "TRANSFORM",
            Self::Clip => "CLIP",
            Self::Scale => "SCALE",
            Self::Loop => "LOOP",
        }
    }

    /// Get the LED for this button
    pub fn led(&self) -> HardwareLED {
        HardwareLED::from(*self)
    }

    pub const fn count() -> usize {
        28
    }

    /// Get all buttons as an array
    pub fn all_buttons() -> [HardwareButton; 28] {
        [
            Self::EncoderFunction1,
            Self::EncoderFunction2,
            Self::EncoderFunction3,
            Self::EncoderFunction4,
            Self::EncoderFunction5,
            Self::EncoderFunction6,
            Self::EncoderFunction7,
            Self::EncoderFunction8,
            Self::Back,
            Self::Load,
            Self::Save,
            Self::Copy,
            Self::Fill,
            Self::Select,
            Self::TapTempo,
            Self::Play,
            Self::Record,
            Self::Shift,
            Self::Time,
            Self::Quantize,
            Self::Automation,
            Self::Transform,
            Self::Session,
            Self::Clip,
            Self::Scope,
            Self::Keyboard,
            Self::Scale,
            Self::Loop,
        ]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ButtonState {
    button: HardwareButton,
    is_pressed: bool,
    has_changed: bool,
    led_state: LEDState,
}

impl ButtonState {
    /// Create a new button with LED state
    pub fn new(button: HardwareButton) -> Self {
        Self {
            button,
            is_pressed: false,
            has_changed: false,
            led_state: LEDState::new(button.led()),
        }
    }

    pub fn button(&self) -> HardwareButton {
        self.button
    }

    pub fn set_pressed(&mut self, pressed: bool) {
        if self.is_pressed != pressed {
            self.is_pressed = pressed;
            self.has_changed = true;
        }
    }

    pub fn is_pressed(&self) -> bool {
        self.is_pressed
    }

    pub fn led_state(&mut self) -> &mut LEDState {
        &mut self.led_state
    }

    pub fn just_pressed(&self) -> bool {
        self.has_changed && self.is_pressed
    }

    pub fn just_released(&self) -> bool {
        self.has_changed && !self.is_pressed
    }

    pub fn clear_changed(&mut self) {
        self.has_changed = false;
    }
}

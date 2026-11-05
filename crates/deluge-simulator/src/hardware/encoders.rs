/// Rotary encoders on the Deluge (7 total)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwareEncoder {
    /// Horizontal navigation encoder (◀▶ directional push)
    HorizontalEncoder,

    /// Vertical navigation encoder (▲▼ directional push)
    VerticalEncoder,

    /// Upper gold encoder (left side)
    UpperGold,

    /// Lower gold encoder (left side)
    LowerGold,

    /// Center select encoder
    Select,

    /// Tempo encoder (with swing, labeled "TEMPO" and "SWING")
    Tempo,

    /// Volume/Output level encoder (far right, labeled "OUTPUT LEVEL")
    Volume,
}

impl HardwareEncoder {
    /// Get the display name for this encoder
    pub fn name(&self) -> &'static str {
        match self {
            Self::HorizontalEncoder => "◀▶",
            Self::VerticalEncoder => "▲▼",
            Self::UpperGold => "UPPER GOLD",
            Self::LowerGold => "LOWER GOLD",
            Self::Select => "SELECT",
            Self::Tempo => "TEMPO",
            Self::Volume => "VOLUME",
        }
    }

    pub const fn count() -> usize {
        7
    }

    /// Get all encoders as an array
    pub fn all_encoders() -> [HardwareEncoder; 7] {
        [
            Self::HorizontalEncoder,
            Self::VerticalEncoder,
            Self::UpperGold,
            Self::LowerGold,
            Self::Select,
            Self::Tempo,
            Self::Volume,
        ]
    }
}

pub struct EncoderState {
    encoder: HardwareEncoder,
    value: i32,
    is_pressed: bool,
    has_changed: bool,
}

impl EncoderState {
    /// Create a new encoder state
    pub fn new(encoder: HardwareEncoder) -> Self {
        Self {
            encoder,
            value: 0,
            is_pressed: false,
            has_changed: false,
        }
    }

    /// Set the encoder value
    pub fn set_value(&mut self, value: i32) {
        if self.value != value {
            self.value = value;
            self.has_changed = true;
        }
    }

    /// Set the pressed state
    pub fn set_pressed(&mut self, pressed: bool) {
        if self.is_pressed != pressed {
            self.is_pressed = pressed;
            self.has_changed = true;
        }
    }

    /// Clear the changed flag
    pub fn clear_changed(&mut self) {
        self.has_changed = false;
    }

    pub fn value(&self) -> i32 {
        self.value
    }

    pub fn is_pressed(&self) -> bool {
        self.is_pressed
    }

    pub fn has_changed(&self) -> bool {
        self.has_changed
    }

    pub fn encoder(&self) -> HardwareEncoder {
        self.encoder
    }
}

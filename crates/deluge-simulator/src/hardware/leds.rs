use crate::hardware::HardwareButton;

/// Indicator LEDs on the Deluge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardwareLED {
    /// Upper gold encoder indicators (4 segments)
    UpperGoldIndicator1,
    UpperGoldIndicator2,
    UpperGoldIndicator3,
    UpperGoldIndicator4,

    /// Lower gold encoder indicators (4 segments)
    LowerGoldIndicator1,
    LowerGoldIndicator2,
    LowerGoldIndicator3,
    LowerGoldIndicator4,

    /// Synced LED (under tempo/swing text)
    Synced,

    // Button LEDs (each button has an integrated LED)
    // Encoder select button LEDs
    EncoderFunction1,
    EncoderFunction2,
    EncoderFunction3,
    EncoderFunction4,
    EncoderFunction5,
    EncoderFunction6,
    EncoderFunction7,
    EncoderFunction8,

    // Top center button LEDs
    Back,
    Load,
    Save,
    Copy,

    // Top right button LEDs
    Fill,
    Select,
    TapTempo,

    // Transport button LEDs
    Play,
    Record,
    Shift,

    // Under-screen row button LEDs
    Time,
    Quantize,
    Automation,
    Transform,

    // Mode button LEDs
    Session,
    Clip,

    // Function button LEDs
    Scope,
    Keyboard,
    Scale,
    Loop,
}

impl HardwareLED {
    /// Get all LEDs
    pub fn all_leds() -> Vec<Self> {
        vec![
            // Encoder indicators
            Self::UpperGoldIndicator1,
            Self::UpperGoldIndicator2,
            Self::UpperGoldIndicator3,
            Self::UpperGoldIndicator4,
            Self::LowerGoldIndicator1,
            Self::LowerGoldIndicator2,
            Self::LowerGoldIndicator3,
            Self::LowerGoldIndicator4,
            Self::Synced,
            // Encoder select button LEDs
            Self::EncoderFunction1,
            Self::EncoderFunction2,
            Self::EncoderFunction3,
            Self::EncoderFunction4,
            Self::EncoderFunction5,
            Self::EncoderFunction6,
            Self::EncoderFunction7,
            Self::EncoderFunction8,
            // Top center button LEDs
            Self::Back,
            Self::Load,
            Self::Save,
            Self::Copy,
            // Top right button LEDs
            Self::Fill,
            Self::Select,
            Self::TapTempo,
            // Transport button LEDs
            Self::Play,
            Self::Record,
            Self::Shift,
            // Mode button LEDs
            Self::Session,
            Self::Clip,
            // Under-screen row button LEDs
            Self::Time,
            Self::Quantize,
            Self::Automation,
            Self::Transform,
            // Function button LEDs
            Self::Scope,
            Self::Keyboard,
            Self::Scale,
            Self::Loop,
        ]
    }

    /// Get the RGB color of this LED (r, g, b) in 0.0-1.0 range
    /// Based on actual Deluge hardware LED colors
    pub fn color(&self) -> (f32, f32, f32) {
        let blue = (0.2, 0.6, 1.0);
        let red = (1.0, 0.0, 0.0);
        let green_yellow = (0.6, 1.0, 0.0);
        let amber = (1.0, 0.6, 0.0);
        match self {
            // Encoder indicators - Red
            Self::UpperGoldIndicator1
            | Self::UpperGoldIndicator2
            | Self::UpperGoldIndicator3
            | Self::UpperGoldIndicator4
            | Self::LowerGoldIndicator1
            | Self::LowerGoldIndicator2
            | Self::LowerGoldIndicator3
            | Self::LowerGoldIndicator4
            | Self::Synced => amber,

            // Transport buttons
            Self::Record => red,        // Red
            Self::Play => green_yellow, // Green

            // Clip type buttons - color coded for easy identification
            Self::Time | Self::Quantize | Self::Automation | Self::Transform => red,

            // Mode buttons
            Self::Session | Self::Clip => blue,

            Self::Keyboard | Self::Scale | Self::Loop => blue,
            Self::Copy | Self::Select | Self::Shift => blue,

            Self::TapTempo => green_yellow,
            Self::Fill => blue,

            // Function buttons - amber (standard)
            Self::Back | Self::Load | Self::Save => red,

            Self::Scope
            | Self::EncoderFunction1
            | Self::EncoderFunction2
            | Self::EncoderFunction3
            | Self::EncoderFunction4
            | Self::EncoderFunction5
            | Self::EncoderFunction6
            | Self::EncoderFunction7
            | Self::EncoderFunction8 => amber,
        }
    }
}

impl From<HardwareButton> for HardwareLED {
    fn from(button: HardwareButton) -> Self {
        match button {
            HardwareButton::EncoderFunction1 => Self::EncoderFunction1,
            HardwareButton::EncoderFunction2 => Self::EncoderFunction2,
            HardwareButton::EncoderFunction3 => Self::EncoderFunction3,
            HardwareButton::EncoderFunction4 => Self::EncoderFunction4,
            HardwareButton::EncoderFunction5 => Self::EncoderFunction5,
            HardwareButton::EncoderFunction6 => Self::EncoderFunction6,
            HardwareButton::EncoderFunction7 => Self::EncoderFunction7,
            HardwareButton::EncoderFunction8 => Self::EncoderFunction8,
            HardwareButton::Back => Self::Back,
            HardwareButton::Load => Self::Load,
            HardwareButton::Save => Self::Save,
            HardwareButton::Copy => Self::Copy,
            HardwareButton::Fill => Self::Fill,
            HardwareButton::Select => Self::Select,
            HardwareButton::TapTempo => Self::TapTempo,
            HardwareButton::Play => Self::Play,
            HardwareButton::Record => Self::Record,
            HardwareButton::Shift => Self::Shift,
            HardwareButton::Time => Self::Time,
            HardwareButton::Quantize => Self::Quantize,
            HardwareButton::Automation => Self::Automation,
            HardwareButton::Transform => Self::Transform,
            HardwareButton::Session => Self::Session,
            HardwareButton::Clip => Self::Clip,
            HardwareButton::Scope => Self::Scope,
            HardwareButton::Keyboard => Self::Keyboard,
            HardwareButton::Scale => Self::Scale,
            HardwareButton::Loop => Self::Loop,
        }
    }
}

/// Standalone indicator LEDs (no button)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndicatorLED {
    /// Upper gold encoder ring indicators (4 segments)
    UpperGoldIndicator1,
    UpperGoldIndicator2,
    UpperGoldIndicator3,
    UpperGoldIndicator4,

    /// Lower gold encoder ring indicators (4 segments)
    LowerGoldIndicator1,
    LowerGoldIndicator2,
    LowerGoldIndicator3,
    LowerGoldIndicator4,

    /// Synced LED (under tempo/swing text)
    Synced,
}

impl IndicatorLED {
    pub const fn count() -> usize {
        9
    }
}

impl From<IndicatorLED> for HardwareLED {
    fn from(indicator: IndicatorLED) -> HardwareLED {
        match indicator {
            IndicatorLED::UpperGoldIndicator1 => HardwareLED::UpperGoldIndicator1,
            IndicatorLED::UpperGoldIndicator2 => HardwareLED::UpperGoldIndicator2,
            IndicatorLED::UpperGoldIndicator3 => HardwareLED::UpperGoldIndicator3,
            IndicatorLED::UpperGoldIndicator4 => HardwareLED::UpperGoldIndicator4,
            IndicatorLED::LowerGoldIndicator1 => HardwareLED::LowerGoldIndicator1,
            IndicatorLED::LowerGoldIndicator2 => HardwareLED::LowerGoldIndicator2,
            IndicatorLED::LowerGoldIndicator3 => HardwareLED::LowerGoldIndicator3,
            IndicatorLED::LowerGoldIndicator4 => HardwareLED::LowerGoldIndicator4,
            IndicatorLED::Synced => HardwareLED::Synced,
        }
    }
}

/// State manager for all button LEDs and indicator LEDs
#[derive(Debug, Clone, PartialEq)]
pub struct LEDState {
    led: HardwareLED,
    led_state: bool,
    changed: bool,
}

impl LEDState {
    pub fn new(led: HardwareLED) -> Self {
        Self {
            led,
            led_state: false,
            changed: false,
        }
    }

    /// Get the hardware LED
    pub fn led(&self) -> HardwareLED {
        self.led
    }

    /// Set the LED state
    pub fn set_state(&mut self, on: bool) {
        if self.led_state != on {
            self.led_state = on;
            self.changed = true;
        }
    }

    pub fn is_on(&self) -> bool {
        self.led_state
    }

    pub fn has_changed(&self) -> bool {
        self.changed
    }

    pub fn clear_changed(&mut self) {
        self.changed = false;
    }
}

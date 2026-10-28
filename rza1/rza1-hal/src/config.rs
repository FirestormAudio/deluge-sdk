//! [`init`](crate::init)'s configuration: the clock tree, the CPG's frequency control, which
//! module clocks run, and the interrupt priorities the HAL assigns itself.
//!
//! Every type here has `const` constructors and builders, so a board can keep its configuration
//! in a `const`.

use crate::clocks::ClockConfig;
use crate::interrupt::Priority;

/// The chip configuration for [`init`](crate::init).
///
/// There is no `Default`: the RZ/A1 has no internal oscillator, so no crystal frequency is a
/// safe guess, and a wrong one would silently mis-time every peripheral. Start from
/// [`Config::new`] with the board's crystal.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Config {
    /// The board's crystal, from which every peripheral clock is derived.
    pub clocks: ClockConfig,
    /// The CPG's frequency control: the CPU clock divider and the CKO pin.
    pub cpg: CpgConfig,
    /// Which module clocks run.
    pub module_clocks: ModuleClocks,
    /// The priority of every DMAC channel's completion interrupt.
    pub dma_interrupt_priority: Priority,
    /// The priority of the OSTM time driver's interrupts (`time-driver-ostm`).
    pub time_interrupt_priority: Priority,
}

impl Config {
    /// A configuration for a board with `clocks`' crystal, everything else at its default: the
    /// CPG and module clocks at their reset settings, and both priorities at
    /// [`Priority::DEFAULT`].
    pub const fn new(clocks: ClockConfig) -> Self {
        Self {
            clocks,
            cpg: CpgConfig::new(),
            module_clocks: ModuleClocks::new(),
            dma_interrupt_priority: Priority::DEFAULT,
            time_interrupt_priority: Priority::DEFAULT,
        }
    }

    /// With `cpg` as the CPG's frequency control.
    pub const fn with_cpg(mut self, cpg: CpgConfig) -> Self {
        self.cpg = cpg;
        self
    }

    /// With `module_clocks` running.
    pub const fn with_module_clocks(mut self, module_clocks: ModuleClocks) -> Self {
        self.module_clocks = module_clocks;
        self
    }

    /// With every DMAC completion interrupt at `priority`.
    pub const fn with_dma_interrupt_priority(mut self, priority: Priority) -> Self {
        self.dma_interrupt_priority = priority;
        self
    }

    /// With the time driver's interrupts at `priority`.
    pub const fn with_time_interrupt_priority(mut self, priority: Priority) -> Self {
        self.time_interrupt_priority = priority;
        self
    }
}

// ---------------------------------------------------------------------------
// FRQCR
// ---------------------------------------------------------------------------

/// The CPU clock (Iφ) as a fraction of the PLL output's maximum (FRQCR.IFC). With the PLL at
/// ×30 on a 13.33 MHz crystal, `Div1` is 400 MHz.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum CpuClockDivider {
    /// ×1 (IFC = 00).
    Div1,
    /// ×2/3 (IFC = 01).
    Div2_3,
    /// ×1/3 (IFC = 11): the reset setting.
    Div1_3,
}

/// What the CKO pin outputs in normal operation, sleep mode and standby (FRQCR.CKOEN).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ClockOutput {
    /// The clock in normal operation; high impedance in sleep and standby (00): the reset
    /// setting.
    HizInStandby,
    /// The clock in normal operation; low in sleep and standby (01).
    LowInStandby,
    /// The clock in normal operation and (unstable) in sleep; low or high in standby (10).
    Always,
    /// High impedance throughout (11).
    Off,
}

/// What the CKO pin outputs while the clock frequency is changing (FRQCR.CKOEN2).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ClockOutputOnChange {
    /// The (unstable) clock (0): the reset setting.
    Unstable,
    /// Low (1).
    Low,
}

/// The CPG's frequency control register (FRQCR). The PLL multiplier is set by the clock-mode
/// pins, not here; FRQCR's reserved bits are written at their fixed values.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct CpgConfig {
    /// The CPU clock divider.
    pub cpu_clock: CpuClockDivider,
    /// The CKO pin in normal operation, sleep and standby.
    pub clock_output: ClockOutput,
    /// The CKO pin while the frequency changes.
    pub clock_output_on_change: ClockOutputOnChange,
}

/// FRQCR's reserved bits 5:4 and 2:0, which read as 0b11_0101 and must be written so.
const FRQCR_FIXED: u16 = 0x35;

impl CpgConfig {
    /// FRQCR's reset settings: CPU clock ×1/3, CKO Hi-Z in standby, unstable CKO during a
    /// frequency change.
    pub const fn new() -> Self {
        Self {
            cpu_clock: CpuClockDivider::Div1_3,
            clock_output: ClockOutput::HizInStandby,
            clock_output_on_change: ClockOutputOnChange::Unstable,
        }
    }

    /// With the CPU clock divided by `divider`.
    pub const fn with_cpu_clock(mut self, divider: CpuClockDivider) -> Self {
        self.cpu_clock = divider;
        self
    }

    /// With the CKO pin set to `output`.
    pub const fn with_clock_output(mut self, output: ClockOutput) -> Self {
        self.clock_output = output;
        self
    }

    /// With the CKO pin set to `output` while the frequency changes.
    pub const fn with_clock_output_on_change(mut self, output: ClockOutputOnChange) -> Self {
        self.clock_output_on_change = output;
        self
    }

    /// The FRQCR value [`init`](crate::init) writes.
    pub const fn frqcr(&self) -> u16 {
        let ifc: u16 = match self.cpu_clock {
            CpuClockDivider::Div1 => 0b00,
            CpuClockDivider::Div2_3 => 0b01,
            CpuClockDivider::Div1_3 => 0b11,
        };
        let ckoen: u16 = match self.clock_output {
            ClockOutput::HizInStandby => 0b00,
            ClockOutput::LowInStandby => 0b01,
            ClockOutput::Always => 0b10,
            ClockOutput::Off => 0b11,
        };
        let ckoen2: u16 = match self.clock_output_on_change {
            ClockOutputOnChange::Unstable => 0,
            ClockOutputOnChange::Low => 1,
        };
        (ckoen2 << 14) | (ckoen << 12) | (ifc << 8) | FRQCR_FIXED
    }
}

impl Default for CpgConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// STBCR2–STBCR12
// ---------------------------------------------------------------------------

macro_rules! module_clocks {
    ($($(#[doc = $doc:literal])* $(#[cfg($cfg:meta)])? $name:ident = ($reg:literal, $bit:literal),)*) => {
        /// A module clock in the CPG's standby control registers (RZ/A1L hardware manual §42.2).
        #[non_exhaustive]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[cfg_attr(feature = "defmt", derive(defmt::Format))]
        pub enum ModuleClock {
            $($(#[doc = $doc])* $(#[cfg($cfg)])? $name,)*
        }

        impl ModuleClock {
            /// Every module clock on this chip.
            pub const ALL: &'static [ModuleClock] = &[$($(#[cfg($cfg)])? ModuleClock::$name,)*];

            /// Its STBCR register number (2–12) and bit: its MSTP bit.
            const fn mstp(self) -> (u8, u8) {
                match self {
                    $($(#[cfg($cfg)])? ModuleClock::$name => ($reg, $bit),)*
                }
            }
        }
    };
}

module_clocks! {
    /// CoreSight (MSTP20): the debug logic. Runs at reset; stopping it cuts off a debugger.
    CoreSight = (2, 0),
    /// The IEBus controller (MSTP37).
    IeBus = (3, 7),
    /// The IrDA serial interface (MSTP36).
    Irda = (3, 6),
    /// LIN interface channel 0 (MSTP35).
    Lin0 = (3, 5),
    /// The multi-function timer pulse unit 2 (MSTP33).
    Mtu2 = (3, 3),
    /// The CAN interface (MSTP32).
    Can = (3, 2),
    /// The A/D converter's clock and its analog power supply (MSTP31). Runs at reset.
    AdcAnalog = (3, 1),
    /// SCIF channel 0 (MSTP47).
    Scif0 = (4, 7),
    /// SCIF channel 1 (MSTP46).
    Scif1 = (4, 6),
    /// SCIF channel 2 (MSTP45).
    Scif2 = (4, 5),
    /// SCIF channel 3 (MSTP44).
    Scif3 = (4, 4),
    /// SCIF channel 4 (MSTP43).
    Scif4 = (4, 3),
    /// SCI channel 0 (MSTP57).
    Sci0 = (5, 7),
    /// SCI channel 1 (MSTP56).
    Sci1 = (5, 6),
    /// OS timer channel 0 (MSTP51).
    Ostm0 = (5, 1),
    /// OS timer channel 1 (MSTP50).
    Ostm1 = (5, 0),
    /// The A/D converter (MSTP67).
    Adc = (6, 7),
    /// The capture engine unit (MSTP66).
    Ceu = (6, 6),
    /// The JPEG codec unit (MSTP61; RZ/A1LU only).
    Jcu = (6, 1),
    /// The realtime clock (MSTP60). Runs at reset; the manual's RTC steps (RCR2.RTCEN = 0,
    /// RCR5.RCKSEL = 00) come before stopping it.
    Rtc = (6, 0),
    /// The Ethernet controller (MSTP74).
    Ether = (7, 4),
    /// USB 2.0 host/function channel 0 (MSTP71).
    Usb0 = (7, 1),
    /// USB 2.0 host/function channel 1 (MSTP70).
    Usb1 = (7, 0),
    /// The MMC host interface (MSTP84).
    Mmcif = (8, 4),
    /// The media local bus (MSTP83).
    Mlb = (8, 3),
    /// EthernetAVB (MSTP82).
    EtherAvb = (8, 2),
    /// SCUX, the sampling-rate converter (MSTP81).
    Scux = (8, 1),
    /// I²C channel 0 (MSTP97).
    I2c0 = (9, 7),
    /// I²C channel 1 (MSTP96).
    I2c1 = (9, 6),
    /// I²C channel 2 (MSTP95).
    I2c2 = (9, 5),
    /// I²C channel 3 (MSTP94).
    I2c3 = (9, 4),
    /// The SPI multi-I/O bus controller, channel 0 (MSTP93).
    Spibsc0 = (9, 3),
    /// Video display controller 5 (MSTP91).
    Vdc5 = (9, 1),
    /// RSPI channel 0 (MSTP107).
    Rspi0 = (10, 7),
    /// RSPI channel 1 (MSTP106).
    Rspi1 = (10, 6),
    /// RSPI channel 2 (MSTP105).
    Rspi2 = (10, 5),
    /// RSPI channel 3 (MSTP104; RZ/A1H).
    #[cfg(feature = "rza1h")]
    Rspi3 = (10, 4),
    /// RSPI channel 4 (MSTP103; RZ/A1H).
    #[cfg(feature = "rza1h")]
    Rspi4 = (10, 3),
    /// The CD-ROM decoder (MSTP102).
    CdRomDecoder = (10, 2),
    /// The SPDIF interface (MSTP101).
    Spdif = (10, 1),
    /// Serial sound interface channel 0 (MSTP115).
    Ssif0 = (11, 5),
    /// Serial sound interface channel 1 (MSTP114).
    Ssif1 = (11, 4),
    /// Serial sound interface channel 2 (MSTP113).
    Ssif2 = (11, 3),
    /// Serial sound interface channel 3 (MSTP112).
    Ssif3 = (11, 2),
    /// SD host interface 00 (MSTP123).
    Sdhi00 = (12, 3),
    /// SD host interface 01 (MSTP122).
    Sdhi01 = (12, 2),
    /// SD host interface 10 (MSTP121).
    Sdhi10 = (12, 1),
    /// SD host interface 11 (MSTP120).
    Sdhi11 = (12, 0),
}

/// STBCR2–STBCR12 at reset: CoreSight, the A/D converter's analog side and the realtime clock
/// run, everything else is stopped, and every reserved bit (and STBCR2's HIZ) holds the value the
/// manual says to write. Only MSTP bits ever change from here.
const RESET_STBCR: [u8; 11] = [0x6A, 0xFD, 0xFF, 0xFF, 0xFE, 0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Which module clocks run (CPG STBCR2–STBCR12). Starts at the reset state; build the set with
/// [`with`](Self::with) and [`without`](Self::without).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ModuleClocks([u8; 11]);

impl ModuleClocks {
    /// The reset state: only CoreSight, the A/D converter's analog side and the realtime clock
    /// run.
    pub const fn new() -> Self {
        Self(RESET_STBCR)
    }

    /// With `module`'s clock running.
    pub const fn with(mut self, module: ModuleClock) -> Self {
        let (reg, bit) = module.mstp();
        self.0[reg as usize - 2] &= !(1 << bit);
        self
    }

    /// With `module`'s clock stopped.
    pub const fn without(mut self, module: ModuleClock) -> Self {
        let (reg, bit) = module.mstp();
        self.0[reg as usize - 2] |= 1 << bit;
        self
    }

    /// Whether `module`'s clock runs.
    pub const fn runs(&self, module: ModuleClock) -> bool {
        let (reg, bit) = module.mstp();
        self.0[reg as usize - 2] & (1 << bit) == 0
    }

    /// The value [`init`](crate::init) writes to STBCR`n` (a 0 bit runs that module's clock).
    ///
    /// # Panics
    /// If `n` is outside 2–12.
    pub const fn stbcr(&self, n: u8) -> u8 {
        assert!(n >= 2 && n <= 12, "STBCR2–STBCR12 only");
        self.0[n as usize - 2]
    }

    /// STBCR2–STBCR12, in order.
    pub(crate) const fn stbcr_all(&self) -> [u8; 11] {
        self.0
    }
}

impl Default for ModuleClocks {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// STBCR2–STBCR12 at reset (RZ/A1L hardware manual §42.2): CoreSight (MSTP20), the A/D
    /// converter's analog side (MSTP31) and the realtime clock (MSTP60) run; everything else is
    /// stopped, and each reserved bit holds its fixed value.
    const RESET_STBCR: [u8; 11] = [0x6A, 0xFD, 0xFF, 0xFF, 0xFE, 0x3F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

    fn stbcr_all(m: &ModuleClocks) -> [u8; 11] {
        core::array::from_fn(|i| m.stbcr(i as u8 + 2))
    }

    #[test]
    fn module_clocks_default_is_the_reset_state() {
        assert_eq!(stbcr_all(&ModuleClocks::default()), RESET_STBCR);
        assert_eq!(stbcr_all(&ModuleClocks::new()), RESET_STBCR);
    }

    #[test]
    fn with_a_module_clears_its_stbcr_bit() {
        // OSTM0 is STBCR5 bit 1 (MSTP51).
        let m = ModuleClocks::new().with(ModuleClock::Ostm0);
        assert_eq!(m.stbcr(5), 0xFF & !(1 << 1));
        assert_eq!(m.stbcr(4), 0xFF, "no other register changes");
    }

    #[test]
    fn without_a_module_sets_its_stbcr_bit() {
        // The realtime clock is STBCR6 bit 0 (MSTP60), running at reset.
        let m = ModuleClocks::new().without(ModuleClock::Rtc);
        assert_eq!(m.stbcr(6), 0xFF);
    }

    #[test]
    fn reserved_bits_keep_their_fixed_values_whatever_runs() {
        // Every module running: only the reserved bits are left, at their fixed values.
        let all = ModuleClock::ALL.iter().fold(ModuleClocks::new(), |m, &c| m.with(c));
        assert_eq!(
            stbcr_all(&all),
            [0x6A, 0x11, 0x07, 0x3C, 0x3C, 0x2C, 0xE1, 0x05, 0x19, 0xC3, 0xF0]
        );
    }

    #[test]
    fn cpg_default_encodes_the_frqcr_reset_value() {
        // FRQCR resets to H'0335: CPU clock ×1/3, CKO off-in-standby (Hi-Z), reserved bits 0x35.
        assert_eq!(CpgConfig::default().frqcr(), 0x0335);
        assert_eq!(CpgConfig::new().frqcr(), 0x0335);
    }

    #[test]
    fn cpg_fields_land_in_their_frqcr_bits() {
        let c = CpgConfig::new()
            .with_cpu_clock(CpuClockDivider::Div1)
            .with_clock_output(ClockOutput::LowInStandby)
            .with_clock_output_on_change(ClockOutputOnChange::Low);
        // CKOEN2 bit 14, CKOEN[1:0] bits 13:12, IFC[1:0] bits 9:8, reserved 0x35.
        assert_eq!(c.frqcr(), (1 << 14) | (0b01 << 12) | 0x35);
    }

    #[test]
    #[should_panic]
    fn stbcr_outside_2_to_12_panics() {
        let _ = ModuleClocks::new().stbcr(13);
    }

    #[test]
    fn config_new_takes_the_defaults() {
        let c = Config::new(crate::clocks::ClockConfig::new(13_333_333));
        assert_eq!(c.cpg, CpgConfig::default());
        assert_eq!(c.module_clocks, ModuleClocks::default());
        assert_eq!(c.dma_interrupt_priority, crate::interrupt::Priority::DEFAULT);
        assert_eq!(c.time_interrupt_priority, crate::interrupt::Priority::DEFAULT);
    }
}

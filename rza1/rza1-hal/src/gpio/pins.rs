//! The pin singletons' [`Pin`](super::Pin) impls: every RZ/A1L pin of Table 41.8 (Number and
//! Function of Ports). `out` marks a pin with a port-mode output ([`OutputCapable`]): every pin
//! but P0_0–P0_3 (input only) and P1_8–P1_15 (input only; P1_0–P1_7 are open-drain outputs).
//! P8 and P9 exist only on the 208- and 233-pin packages.

use super::{AnyPin, OutputCapable, Pin, Port, sealed};

macro_rules! output_capable {
    ($name:ident, out) => {
        impl OutputCapable for crate::peripherals::$name {}
    };
    ($name:ident, in) => {};
}

macro_rules! pins {
    ($($name:ident = ($port:literal, $bit:literal, $dir:ident),)*) => {$(
        impl sealed::Sealed for crate::peripherals::$name {}
        impl Pin for crate::peripherals::$name {
            #[inline]
            fn port(&self) -> Port {
                Port::from_index($port)
            }
            #[inline]
            fn bit(&self) -> u8 {
                $bit
            }
        }
        impl From<crate::peripherals::$name> for AnyPin {
            #[inline]
            fn from(_: crate::peripherals::$name) -> Self {
                AnyPin { port: $port, bit: $bit }
            }
        }
        output_capable!($name, $dir);
    )*};
}

pins! {
    P0_0 = (0, 0, in),
    P0_1 = (0, 1, in),
    P0_2 = (0, 2, in),
    P0_3 = (0, 3, in),
    P1_0 = (1, 0, out),
    P1_1 = (1, 1, out),
    P1_2 = (1, 2, out),
    P1_3 = (1, 3, out),
    P1_4 = (1, 4, out),
    P1_5 = (1, 5, out),
    P1_6 = (1, 6, out),
    P1_7 = (1, 7, out),
    P1_8 = (1, 8, in),
    P1_9 = (1, 9, in),
    P1_10 = (1, 10, in),
    P1_11 = (1, 11, in),
    P1_12 = (1, 12, in),
    P1_13 = (1, 13, in),
    P1_14 = (1, 14, in),
    P1_15 = (1, 15, in),
    P2_0 = (2, 0, out),
    P2_1 = (2, 1, out),
    P2_2 = (2, 2, out),
    P2_3 = (2, 3, out),
    P2_4 = (2, 4, out),
    P2_5 = (2, 5, out),
    P2_6 = (2, 6, out),
    P2_7 = (2, 7, out),
    P2_8 = (2, 8, out),
    P2_9 = (2, 9, out),
    P3_0 = (3, 0, out),
    P3_1 = (3, 1, out),
    P3_2 = (3, 2, out),
    P3_3 = (3, 3, out),
    P3_4 = (3, 4, out),
    P3_5 = (3, 5, out),
    P3_6 = (3, 6, out),
    P3_7 = (3, 7, out),
    P3_8 = (3, 8, out),
    P3_9 = (3, 9, out),
    P3_10 = (3, 10, out),
    P3_11 = (3, 11, out),
    P3_12 = (3, 12, out),
    P3_13 = (3, 13, out),
    P3_14 = (3, 14, out),
    P3_15 = (3, 15, out),
    P4_0 = (4, 0, out),
    P4_1 = (4, 1, out),
    P4_2 = (4, 2, out),
    P4_3 = (4, 3, out),
    P4_4 = (4, 4, out),
    P4_5 = (4, 5, out),
    P4_6 = (4, 6, out),
    P4_7 = (4, 7, out),
    P5_0 = (5, 0, out),
    P5_1 = (5, 1, out),
    P5_2 = (5, 2, out),
    P5_3 = (5, 3, out),
    P5_4 = (5, 4, out),
    P5_5 = (5, 5, out),
    P5_6 = (5, 6, out),
    P5_7 = (5, 7, out),
    P5_8 = (5, 8, out),
    P5_9 = (5, 9, out),
    P5_10 = (5, 10, out),
    P5_11 = (5, 11, out),
    P5_12 = (5, 12, out),
    P5_13 = (5, 13, out),
    P5_14 = (5, 14, out),
    P5_15 = (5, 15, out),
    P6_0 = (6, 0, out),
    P6_1 = (6, 1, out),
    P6_2 = (6, 2, out),
    P6_3 = (6, 3, out),
    P6_4 = (6, 4, out),
    P6_5 = (6, 5, out),
    P6_6 = (6, 6, out),
    P6_7 = (6, 7, out),
    P6_8 = (6, 8, out),
    P6_9 = (6, 9, out),
    P6_10 = (6, 10, out),
    P6_11 = (6, 11, out),
    P6_12 = (6, 12, out),
    P6_13 = (6, 13, out),
    P6_14 = (6, 14, out),
    P6_15 = (6, 15, out),
    P7_0 = (7, 0, out),
    P7_1 = (7, 1, out),
    P7_2 = (7, 2, out),
    P7_3 = (7, 3, out),
    P7_4 = (7, 4, out),
    P7_5 = (7, 5, out),
    P7_6 = (7, 6, out),
    P7_7 = (7, 7, out),
    P7_8 = (7, 8, out),
    P7_9 = (7, 9, out),
    P7_10 = (7, 10, out),
    P7_11 = (7, 11, out),
    P8_0 = (8, 0, out),
    P8_1 = (8, 1, out),
    P8_2 = (8, 2, out),
    P8_3 = (8, 3, out),
    P8_4 = (8, 4, out),
    P8_5 = (8, 5, out),
    P8_6 = (8, 6, out),
    P8_7 = (8, 7, out),
    P8_8 = (8, 8, out),
    P8_9 = (8, 9, out),
    P8_10 = (8, 10, out),
    P8_11 = (8, 11, out),
    P8_12 = (8, 12, out),
    P8_13 = (8, 13, out),
    P8_14 = (8, 14, out),
    P8_15 = (8, 15, out),
    P9_0 = (9, 0, out),
    P9_1 = (9, 1, out),
    P9_2 = (9, 2, out),
    P9_3 = (9, 3, out),
    P9_4 = (9, 4, out),
    P9_5 = (9, 5, out),
}

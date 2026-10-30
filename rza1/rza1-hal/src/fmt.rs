//! Logging macros over `log`, `defmt` or nothing, after embassy-hal-internal's `fmt.rs`: the
//! crate logs through `trace!` … `error!`, and the `log` / `defmt` feature picks the backend. With
//! neither, the macros still evaluate (and so use) their arguments, and emit nothing.
//!
//! Format strings must suit both backends: positional `{}` arguments only (no inline `{name}`
//! captures, which `defmt` doesn't take and the no-backend arm couldn't see), and only the
//! `{:?}`/`{:x}`-style specifiers `defmt` understands.
#![macro_use]
#![allow(unused)]

#[cfg(all(feature = "defmt", feature = "log"))]
compile_error!("rza1-hal: enable at most one of the `log` and `defmt` features");

#[collapse_debuginfo(yes)]
macro_rules! trace {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::trace!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::trace!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::debug!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::debug!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! info {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::info!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::info!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! warn {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::warn!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::warn!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! error {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::error!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::error!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

/// A byte slice that logs as two-digit hex (`[80, 06, 00, 01]`) under `log` and `defmt` alike:
/// no format specifier does that for a slice on both backends.
pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl core::fmt::Debug for Bytes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("[")?;
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{b:02x}")?;
        }
        f.write_str("]")
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for Bytes<'_> {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(f, "{=[u8]:02x}", self.0)
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::Bytes;
    use std::format;

    #[test]
    fn bytes_debug_prints_two_digit_hex() {
        assert_eq!(
            format!("{:?}", Bytes(&[0x80, 0x06, 0x00, 0x01])),
            "[80, 06, 00, 01]"
        );
        assert_eq!(format!("{:?}", Bytes(&[])), "[]");
    }
}

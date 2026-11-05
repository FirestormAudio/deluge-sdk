//! The `log` backend over RTT. rtt-target's own logger formats each record inside its critical
//! section, so every log line held interrupts off for as long as formatting took (tens of
//! microseconds for a typical line). This one formats into a stack buffer first, with interrupts
//! on, and holds the critical section only to copy the finished line into RTT channel 0
//! (rtt-target's print channel, which `rprintln!` and the fault paths share). Lines stay whole and
//! in order; one longer than [`LINE_BYTES`] is cut and ends with `…`.

use core::fmt::{self, Write};

/// The longest line, newline included.
pub(crate) const LINE_BYTES: usize = 256;

/// `…` and the newline a cut line ends with.
const CUT_MARK: &str = "…\n";

/// A line being formatted: keeps what fits, leaving room for the newline, and whether it cut.
struct Line<'a> {
    out: &'a mut [u8],
    len: usize,
    cut: bool,
}

impl Write for Line<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.cut {
            return Ok(());
        }
        let room = self.out.len() - 1 - self.len;
        let mut take = s.len().min(room);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.out[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        self.cut = take < s.len();
        Ok(())
    }
}

/// Format a record as rtt-target did, `"{level:<5} [{target}] {args}\n"`, into `out`; the length.
/// A record that doesn't fit ends with `…` and the newline instead.
pub(crate) fn format_record(
    level: log::Level,
    target: &str,
    args: fmt::Arguments,
    out: &mut [u8],
) -> usize {
    let mut line = Line {
        out,
        len: 0,
        cut: false,
    };
    let _ = write!(line, "{level:<5} [{target}] {args}");
    let end = if line.cut { CUT_MARK } else { "\n" };
    let mut n = line.len;
    if line.cut {
        // Room for the mark, backing off to a character boundary (a UTF-8 continuation byte is
        // 0b10xx_xxxx).
        n = n.min(line.out.len() - CUT_MARK.len());
        while n > 0 && line.out[n] & 0xC0 == 0x80 {
            n -= 1;
        }
    }
    line.out[n..n + end.len()].copy_from_slice(end.as_bytes());
    n + end.len()
}

#[cfg(feature = "rtt")]
mod logger {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::{LINE_BYTES, format_record};

    struct Logger {
        level: AtomicUsize,
    }

    static LOGGER: Logger = Logger {
        level: AtomicUsize::new(0),
    };

    impl log::Log for Logger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() as usize <= self.level.load(Ordering::Relaxed)
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            // Formatted with interrupts on; only the copy below holds the critical section.
            let mut buf = [0u8; LINE_BYTES];
            let n = format_record(record.level(), record.target(), *record.args(), &mut buf);
            // `format_record` writes UTF-8 and cuts only at char boundaries.
            let line = core::str::from_utf8(&buf[..n]).unwrap_or("?\n");
            rtt_target::with_terminal_channel(|t| {
                use core::fmt::Write;
                let _ = t.write(0).write_str(line);
            });
        }

        fn flush(&self) {}
    }

    /// Register the backend at `level`. rtt-target's print channel must be set already
    /// (`rtt_target::set_print_channel`).
    pub(crate) fn init(level: log::LevelFilter) {
        LOGGER.level.store(level as usize, Ordering::Relaxed);
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(level);
        }
    }
}

#[cfg(feature = "rtt")]
pub(crate) use logger::init;

#[cfg(test)]
mod tests {
    use super::*;

    fn line(level: log::Level, target: &str, args: core::fmt::Arguments) -> std::vec::Vec<u8> {
        let mut out = [0u8; LINE_BYTES];
        let n = format_record(level, target, args, &mut out);
        out[..n].to_vec()
    }

    /// The layout rtt-target's logger used, which existing RTT tooling's regexes match.
    #[test]
    fn formats_like_rtt_target() {
        assert_eq!(
            line(log::Level::Info, "usb_log", format_args!("ready")),
            b"INFO  [usb_log] ready\n"
        );
        assert_eq!(
            line(log::Level::Warn, "a::b", format_args!("x {}", 1)),
            b"WARN  [a::b] x 1\n"
        );
    }

    /// A record that fits whole, its newline in the last byte, isn't cut.
    #[test]
    fn a_line_that_just_fits_is_whole() {
        // "INFO  [t] " is 10 bytes; 245 more and the newline make 256.
        let msg = "x".repeat(LINE_BYTES - 11);
        let l = line(log::Level::Info, "t", format_args!("{msg}"));
        assert_eq!(l.len(), LINE_BYTES);
        assert!(
            l.ends_with(b"x\n"),
            "cut: {:?}",
            core::str::from_utf8(&l[240..])
        );
    }

    /// A record longer than the line is cut, and says so, still a whole line of valid UTF-8.
    #[test]
    fn long_record_is_cut_and_marked() {
        let long = "é".repeat(200); // 400 bytes, two per character
        let l = line(log::Level::Info, "t", format_args!("{long}"));
        assert!(l.len() <= LINE_BYTES);
        assert!(l.starts_with(b"INFO  [t] "));
        assert!(l.ends_with("…\n".as_bytes()));
        assert!(core::str::from_utf8(&l).is_ok());
    }
}

//! `cargo deluge log`: connect to a running app's USB serial-log channel and
//! stream it to stdout. The streaming half is shared with `run --log`.

use std::io::{Read, Write};
use std::time::Duration;

use crate::util::arg_value;
use crate::{DELUGE_PID, DELUGE_VID, LOG_PRODUCT, UPLOAD_PRODUCT};

/// `cargo deluge log`: connect to a running app's USB serial-log channel (the
/// SDK's `usb-log` feature, `deluge::usb_debug`) and stream it to stdout until
/// interrupted (Ctrl-C). With `--port` the discovery step is skipped.
pub(crate) fn cmd_log(args: &[String]) -> Result<(), String> {
    let path = match arg_value(args, "--port") {
        Some(p) => p,
        None => {
            println!("log: waiting for the app's USB log port to appear (Ctrl-C to stop)…");
            wait_for_log_port()?
        }
    };
    stream_log(&path)
}

/// Reconnect to the app's USB log port after it re-enumerates (used by
/// `run --log`) and stream it to stdout.
pub(crate) fn tail_log() -> Result<(), String> {
    println!("--log: waiting for the app's USB log port to appear (Ctrl-C to stop)…");
    let path = wait_for_log_port()?;
    stream_log(&path)
}

/// Poll for the SDK USB-log CDC port to (re)appear for a few seconds, preferring
/// the log product string so it isn't confused with the dev-upload listener
/// (which shares the same VID/PID). Falls back to a lone non-upload Deluge port.
fn wait_for_log_port() -> Result<String, String> {
    use serialport::SerialPortType;
    for _ in 0..50 {
        let deluge: Vec<(String, Option<String>)> = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| match p.port_type {
                SerialPortType::UsbPort(info)
                    if info.vid == DELUGE_VID && info.pid == DELUGE_PID =>
                {
                    Some((p.port_name, info.product))
                }
                _ => None,
            })
            .collect();

        // Prefer the SDK-log product string.
        if let Some((name, _)) = deluge
            .iter()
            .find(|(_, prod)| prod.as_deref().is_some_and(|s| s.contains(LOG_PRODUCT)))
        {
            return Ok(name.clone());
        }
        // Otherwise accept a single Deluge port, as long as it isn't the
        // dev-upload listener (which would just sit there silently).
        if let [(name, prod)] = deluge.as_slice()
            && !prod.as_deref().is_some_and(|s| s.contains(UPLOAD_PRODUCT))
        {
            return Ok(name.clone());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(
        "no Deluge SDK-log port appeared. Make sure an app built with the `usb-log` \
         feature is running and connected over USB; pass --port <path> to override."
            .to_string(),
    )
}

/// Open `path` and stream its bytes to stdout until interrupted (Ctrl-C).
fn stream_log(path: &str) -> Result<(), String> {
    let mut port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(500))
        .open()
        .map_err(|e| format!("opening {path}: {e}"))?;
    println!("log: tailing {path}");

    let mut buf = [0u8; 512];
    let mut out = std::io::stdout();
    loop {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                let _ = out.write_all(&buf[..n]);
                let _ = out.flush();
            }
            // Read timeouts are expected when the app is idle; keep waiting.
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("reading log port: {e}")),
        }
    }
}

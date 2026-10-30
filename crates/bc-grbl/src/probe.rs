//! Finding G-code laser controllers on serial ports ("Find My Laser"): open a port, wake the board,
//! and recognise its firmware from what it says.

use crate::gcode::Dialect;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// A controller found on a port.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub port: String,
    pub baud: u32,
    pub dialect: Dialect,
    /// What the board called itself, e.g. "Grbl 1.1h".
    pub info: String,
}

/// Speeds tried, most common first (460800 for LaserPecker and some ESP32 boards).
pub const PROBE_BAUDS: [u32; 5] = [115_200, 460_800, 250_000, 921_600, 57_600];

/// How long to listen at each speed.
const LISTEN: Duration = Duration::from_millis(1600);

/// The firmware a board's replies show, with the line that showed it.
pub fn identify(text: &str) -> Option<(Dialect, String)> {
    for line in text.lines().map(str::trim) {
        let lower = line.to_ascii_lowercase();
        // GRBL greets with "Grbl 1.1h ['$' for help]"; grblHAL and FluidNC also speak GRBL.
        if lower.starts_with("grbl") || lower.contains("grblhal") || lower.contains("fluidnc") {
            let info = line.split(" [").next().unwrap_or(line).trim().to_string();
            return Some((Dialect::Grbl, info));
        }
        if let Some(rest) = line.find("FIRMWARE_NAME:").map(|i| &line[i + "FIRMWARE_NAME:".len()..]) {
            if rest.to_ascii_lowercase().starts_with("marlin") {
                let info = rest.split(" SOURCE_CODE_URL").next().unwrap_or(rest).trim().to_string();
                return Some((Dialect::Marlin, info));
            }
            if rest.to_ascii_lowercase().contains("smoothie") {
                return Some((Dialect::Smoothieware, "Smoothieware".into()));
            }
        }
        if lower.contains("smoothie") {
            return Some((Dialect::Smoothieware, line.to_string()));
        }
    }
    None
}

/// Tries each speed on `port` and returns the controller found, if any. Opening a port resets
/// most Arduino-style boards, which then greet; others are asked who they are. An error means the
/// port couldn't be opened at all (busy, gone, or no permission).
pub fn probe(port: &str) -> Result<Option<Found>, String> {
    for baud in PROBE_BAUDS {
        let mut serial = match serialport::new(port, baud).timeout(Duration::from_millis(50)).open() {
            Ok(s) => s,
            // Busy, gone or not allowed: no point trying other speeds.
            Err(e) => return Err(open_error(&e)),
        };
        let _ = serial.write_data_terminal_ready(true);
        let start = Instant::now();
        let mut heard = Vec::new();
        let mut asked = false;
        while start.elapsed() < LISTEN {
            if !asked && start.elapsed() > Duration::from_millis(400) {
                // Soft-reset (GRBL), then ask Marlin and Smoothieware for their names. Each
                // firmware ignores or rejects the others' requests harmlessly.
                let _ = serial.write_all(b"\r\n\x18");
                let _ = serial.write_all(b"M115\nversion\n");
                let _ = serial.flush();
                asked = true;
            }
            let mut buf = [0u8; 256];
            match serial.read(&mut buf) {
                Ok(n) if n > 0 => {
                    heard.extend_from_slice(&buf[..n]);
                    if let Some((dialect, info)) = identify(&String::from_utf8_lossy(&heard)) {
                        return Ok(Some(Found { port: port.to_string(), baud, dialect, info }));
                    }
                }
                _ => crate::stream::nothing_read(),
            }
        }
    }
    Ok(None)
}

/// Why a port couldn't be opened, in plain words.
pub fn open_error(e: &serialport::Error) -> String {
    let text = e.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("permission denied") || lower.contains("eacces") {
        "no permission to open it (on Linux, add yourself to the dialout group: sudo usermod -aG dialout $USER, then log out and in)".into()
    } else if lower.contains("busy") || lower.contains("access is denied") {
        "in use by another program".into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_firmware_greetings() {
        assert_eq!(identify("\r\nGrbl 1.1h ['$' for help]\r\n"), Some((Dialect::Grbl, "Grbl 1.1h".into())));
        assert_eq!(identify("GrblHAL 1.1f ['$' or '$HELP' for help]").map(|f| f.0), Some(Dialect::Grbl));
        assert_eq!(identify("start\necho:Marlin\nFIRMWARE_NAME:Marlin 2.1.2.1 (Jun 1 2024) SOURCE_CODE_URL:github.com").unwrap(), (Dialect::Marlin, "Marlin 2.1.2.1 (Jun 1 2024)".into()));
        assert_eq!(identify("Build version: edge-1234, Build date: x, MCU: LPC1769, System Clock: 100MHz\nSmoothieware").map(|f| f.0), Some(Dialect::Smoothieware));
        assert_eq!(identify("ok\nerror:20\n"), None);
        assert_eq!(identify(""), None);
    }

    #[test]
    fn explains_ports_that_cannot_be_opened() {
        let denied = serialport::Error::new(serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied), "Permission denied");
        assert!(open_error(&denied).contains("dialout"));
        let busy = serialport::Error::new(serialport::ErrorKind::NoDevice, "Device or resource busy");
        assert_eq!(open_error(&busy), "in use by another program");
    }
}

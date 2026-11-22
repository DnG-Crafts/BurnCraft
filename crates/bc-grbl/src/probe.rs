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
    /// GRBL: how far the head can travel (mm), from `$130` and `$131`.
    pub travel: Option<(f64, f64)>,
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

/// The X and Y travel (mm) in GRBL's settings listing (`$130=381.000`, `$131=305.000`).
pub fn travel(text: &str) -> Option<(f64, f64)> {
    let value = |key: &str| {
        text.lines().map(str::trim).find_map(|l| l.strip_prefix(key)).and_then(|v| v.split(|c: char| c.is_whitespace() || c == '(').next()?.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0)
    };
    Some((value("$130=")?, value("$131=")?))
}

/// Asks a GRBL board for its settings (`$$`) and reads its travel from them.
fn ask_travel(serial: &mut Box<dyn serialport::SerialPort>) -> Option<(f64, f64)> {
    let _ = serial.write_all(b"$$\n");
    let _ = serial.flush();
    let start = Instant::now();
    let mut heard = Vec::new();
    while start.elapsed() < LISTEN {
        let mut buf = [0u8; 512];
        match serial.read(&mut buf) {
            Ok(n) if n > 0 => {
                heard.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&heard);
                if text.contains("$131=") && text.lines().any(|l| l.trim() == "ok") {
                    break;
                }
            }
            _ => crate::stream::nothing_read(),
        }
    }
    travel(&String::from_utf8_lossy(&heard))
}

/// Reads a GRBL board's travel (mm, from `$130` and `$131`) on `port`. Opening the port resets
/// most boards, so this waits for it to start before asking.
pub fn read_travel(port: &str, baud: u32) -> Result<(f64, f64), String> {
    let mut serial = serialport::new(port, baud).timeout(Duration::from_millis(50)).open().map_err(|e| open_error(&e))?;
    let _ = serial.write_data_terminal_ready(true);
    // Let a board that resets on opening finish starting (it greets when it's ready).
    let start = Instant::now();
    let mut heard = Vec::new();
    while start.elapsed() < LISTEN && identify(&String::from_utf8_lossy(&heard)).is_none() {
        let mut buf = [0u8; 256];
        match serial.read(&mut buf) {
            Ok(n) if n > 0 => heard.extend_from_slice(&buf[..n]),
            _ => crate::stream::nothing_read(),
        }
    }
    ask_travel(&mut serial).ok_or_else(|| "the controller didn't report its travel ($130 and $131). Is it a GRBL board, at this baud rate?".to_string())
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
                        // GRBL also says how far it can travel.
                        let travel = if dialect == Dialect::Grbl { ask_travel(&mut serial) } else { None };
                        return Ok(Some(Found { port: port.to_string(), baud, dialect, info, travel }));
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

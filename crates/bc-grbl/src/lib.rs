//! GRBL (and GRBL-compatible) laser controllers over USB serial.
//!
//! Based on the public GRBL v1.1 documentation: laser mode (`$32=1`) with
//! dynamic power (`M4`), character-counting flow control, real-time commands
//! (`?`, `!`, `~`, Ctrl-X) and `$J=` jogging.

pub mod gcode;
pub mod probe;
pub mod protocol;
pub mod settings;
pub mod stream;

pub use gcode::{generate, CustomGcode, Dialect, GcodeOptions, GrblLayer, GrblPath};
pub use protocol::{parse_line, Response, Status};
pub use stream::{Grbl, GrblError, StreamProgress};

/// A serial port, with the USB chip behind it when there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    pub name: String,
    /// USB vendor and product IDs.
    pub usb: Option<(u16, u16)>,
    /// What the port calls itself, e.g. "USB-SERIAL CH340".
    pub description: String,
}

impl PortInfo {
    /// FTDI chips, which Ruida controllers use for USB.
    pub fn is_ftdi(&self) -> bool {
        self.usb.is_some_and(|(vid, _)| vid == 0x0403)
    }

    /// WCH CH340/CH341 chips, used by many diode lasers and the LaserPecker 2.
    pub fn is_ch340(&self) -> bool {
        self.usb.is_some_and(|(vid, _)| vid == 0x1A86)
    }
}

/// Serial ports with what's known about them.
pub fn list_port_info() -> Vec<PortInfo> {
    serialport::available_ports()
        .map(|ports| {
            ports
                .into_iter()
                .map(|p| match p.port_type {
                    serialport::SerialPortType::UsbPort(u) => PortInfo {
                        name: p.port_name,
                        usb: Some((u.vid, u.pid)),
                        description: u.product.or(u.manufacturer).unwrap_or_default(),
                    },
                    _ => PortInfo { name: p.port_name, usb: None, description: String::new() },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Serial ports currently present, by name (e.g. "COM3").
pub fn list_ports() -> Vec<String> {
    serialport::available_ports().map(|ports| ports.into_iter().map(|p| p.port_name).collect()).unwrap_or_default()
}

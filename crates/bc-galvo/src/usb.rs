//! USB transport for BJJCZ boards (VID 0x9588, PID 0x9899).
//!
//! On Windows the board needs the WinUSB driver (install it with Zadig) instead of the
//! EZCad "LMCV2" driver.

use std::time::Duration;

pub const VID: u16 = 0x9588;
pub const PID: u16 = 0x9899;
const EP_WRITE: u8 = 0x02;
const EP_READ: u8 = 0x88;
const TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum UsbError {
    NotFound,
    Usb(String),
}

impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsbError::NotFound => write!(f, "no galvo controller (USB 9588:9899) found; on Windows install the WinUSB driver with Zadig"),
            UsbError::Usb(e) => write!(f, "USB error: {e}"),
        }
    }
}

impl std::error::Error for UsbError {}

/// Raw bulk I/O; implemented by the real board and by a simulator in tests.
pub trait UsbIo: Send {
    fn write(&mut self, data: &[u8]) -> Result<(), UsbError>;
    /// Reads an 8-byte reply.
    fn read(&mut self) -> Result<[u8; 8], UsbError>;
}

pub struct RusbIo {
    handle: rusb::DeviceHandle<rusb::GlobalContext>,
}

impl RusbIo {
    /// Opens the `index`-th board on the bus.
    pub fn open(index: usize) -> Result<Self, UsbError> {
        let devices = rusb::devices().map_err(|e| UsbError::Usb(e.to_string()))?;
        let device = devices
            .iter()
            .filter(|d| d.device_descriptor().map(|desc| desc.vendor_id() == VID && desc.product_id() == PID).unwrap_or(false))
            .nth(index)
            .ok_or(UsbError::NotFound)?;
        let handle = device.open().map_err(|e| UsbError::Usb(e.to_string()))?;
        let _ = handle.set_auto_detach_kernel_driver(true);
        let _ = handle.set_active_configuration(1);
        handle.claim_interface(0).map_err(|e| UsbError::Usb(e.to_string()))?;
        Ok(Self { handle })
    }

    /// How many boards are connected.
    pub fn count() -> usize {
        rusb::devices()
            .map(|list| list.iter().filter(|d| d.device_descriptor().map(|desc| desc.vendor_id() == VID && desc.product_id() == PID).unwrap_or(false)).count())
            .unwrap_or(0)
    }
}

impl Drop for RusbIo {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(0);
    }
}

impl UsbIo for RusbIo {
    fn write(&mut self, data: &[u8]) -> Result<(), UsbError> {
        let mut sent = 0;
        while sent < data.len() {
            sent += self.handle.write_bulk(EP_WRITE, &data[sent..], TIMEOUT * 10).map_err(|e| UsbError::Usb(e.to_string()))?;
        }
        Ok(())
    }

    fn read(&mut self) -> Result<[u8; 8], UsbError> {
        let mut buf = [0u8; 8];
        self.handle.read_bulk(EP_READ, &mut buf, TIMEOUT * 10).map_err(|e| UsbError::Usb(e.to_string()))?;
        Ok(buf)
    }
}

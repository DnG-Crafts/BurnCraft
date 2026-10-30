//! Galvo (BJJCZ / LMC "EZCad2" board) laser support over USB.
//!
//! Implemented from public sources: the open-source balor project and MeerK40t's galvo driver.

pub mod controller;
pub mod job;
pub mod protocol;
pub mod usb;

pub use controller::{Controller, GalvoStatus};
pub use job::{encode_job, GalvoLayer, GalvoSettings, MarkSettings, Source};
pub use usb::{RusbIo, UsbError, UsbIo};

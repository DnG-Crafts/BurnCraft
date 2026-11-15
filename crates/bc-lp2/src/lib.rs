//! LaserPecker 2 control over USB.
//!
//! The LP2 has a CH340 USB serial chip (VID 1A86, PID 7523) and runs at 460800 baud. Commands and
//! replies are framed binary ([`protocol`]); a job is uploaded as a file (a 64-byte header, then
//! G-code text for line work or one byte per pixel for engraving, see [`job`]) and then started
//! with its power and speed. Everything here was worked out from USB captures of the LP2's own
//! app talking to the machine.

pub mod controller;
pub mod job;
pub mod protocol;
pub mod settings;

pub use controller::{Controller, Error, Port};
pub use job::{File, Kind, Resolution};
pub use protocol::{JobState, PreviewKind, State, Status};
pub use settings::{Settings, WorkMode};

/// The baud rate the LP2's app sets on the CH340.
pub const BAUD: u32 = 460_800;

/// The LP2's marking field (mm). Coordinates are measured from its top-left corner.
pub const FIELD: (f64, f64) = (100.0, 100.0);

/// The field with the Versatile Electric Roller (mm): the roller carries the work along Y, so a
/// job can be 2 m long, as in the LP2 app.
pub const ROLLER_FIELD: (f64, f64) = (100.0, 2000.0);

/// The object diameter (mm) sent with previews and jobs. The LP2 app only lets it be set for the
/// gripper accessory (other models); for the LP2 it always sends this default.
pub const OBJECT_DIAMETER: f64 = 20.0;

/// A nominal top marking speed (mm/s). BurnCraft keeps speeds in mm/s, while the LP2 takes a
/// percentage of its top speed; this is what 100 % stands for.
pub const MAX_SPEED: f64 = 600.0;

/// The speed percentage (1-100) sent to the machine for a speed in mm/s.
pub fn speed_percent(mm_s: f64) -> u8 {
    (mm_s / MAX_SPEED * 100.0).round().clamp(1.0, 100.0) as u8
}

/// The LP2 app's "depth" (1-100 %) for a speed in mm/s: deeper is slower. The app sent a 30 %
/// depth as speed 71, so depth is 101 minus the speed percentage.
pub fn depth(mm_s: f64) -> f64 {
    101.0 - speed_percent(mm_s) as f64
}

/// The speed (mm/s) for a depth (1-100 %).
pub fn speed_for_depth(depth: f64) -> f64 {
    (101.0 - depth.round().clamp(1.0, 100.0)) / 100.0 * MAX_SPEED
}

/// The power percentage (1-100) sent to the machine.
pub fn power_percent(power: f64) -> u8 {
    power.round().clamp(1.0, 100.0) as u8
}

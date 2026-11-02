//! The machine's own settings (the LP2 app's "Mode setting" page), written with the `06 01`
//! command. The app writes them each time it connects and whenever one is changed; the machine
//! answers `06 01`.

use serde::{Deserialize, Serialize};

/// How the LP2 is set up for the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WorkMode {
    /// On its stand, over a flat surface.
    #[default]
    Flat,
    /// Engraving round objects.
    Cylinder,
    Slab,
    Trolley,
}

impl WorkMode {
    pub const ALL: [WorkMode; 4] = [WorkMode::Flat, WorkMode::Cylinder, WorkMode::Slab, WorkMode::Trolley];

    pub fn label(self) -> &'static str {
        match self {
            WorkMode::Flat => "Flat surface",
            WorkMode::Cylinder => "Cylinder",
            WorkMode::Slab => "Slab",
            WorkMode::Trolley => "Trolley",
        }
    }

    /// Bytes 7 and 8 of the settings command. Slab and trolley send the same bytes.
    fn bytes(self) -> [u8; 2] {
        match self {
            WorkMode::Flat => [0, 1],
            WorkMode::Cylinder => [1, 1],
            WorkMode::Slab | WorkMode::Trolley => [1, 0],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: WorkMode,
    /// Lets the laser fire without the protective cover in place.
    pub free_mode: bool,
    /// Beeps when work starts and ends.
    pub beep: bool,
    /// Orders vector paths for a quicker job. (Always on in the captures; its byte is assumed.)
    pub path_optimization: bool,
    /// Engraves in reverse, for handheld work.
    pub reverse_direction: bool,
    /// The preview and stop touch buttons on the laser unit.
    pub touch_buttons: bool,
    /// The "Batch Engraving Button" accessory.
    pub batch_button: bool,
}

impl Default for Settings {
    /// As the LP2 app sets a new machine up.
    fn default() -> Self {
        Self { mode: WorkMode::Flat, free_mode: false, beep: true, path_optimization: true, reverse_direction: false, touch_buttons: true, batch_button: false }
    }
}

impl Settings {
    /// The settings command (payload).
    pub fn command(&self) -> Vec<u8> {
        let mut p = vec![0u8; 21];
        p[0] = 0x06;
        p[1] = 0x01;
        p[2] = self.free_mode as u8;
        p[3] = self.beep as u8;
        p[4] = self.path_optimization as u8;
        p[7..9].copy_from_slice(&self.mode.bytes());
        p[9] = self.touch_buttons as u8;
        p[11] = self.batch_button as u8;
        p[18] = self.reverse_direction as u8;
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn each_switch_matches_the_captures() {
        let base = Settings::default();
        let cases = [
            (base, "aa bb 17 06 01 00 01 01 00 00 00 01 01 00 00 00 00 00 00 00 00 00 00 00 00 0b"),
            (Settings { beep: false, ..base }, "aa bb 17 06 01 00 00 01 00 00 00 01 01 00 00 00 00 00 00 00 00 00 00 00 00 0a"),
            (Settings { batch_button: true, ..base }, "aa bb 17 06 01 00 01 01 00 00 00 01 01 00 01 00 00 00 00 00 00 00 00 00 00 0c"),
            (Settings { touch_buttons: false, ..base }, "aa bb 17 06 01 00 01 01 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 00 00 0a"),
            (Settings { reverse_direction: true, ..base }, "aa bb 17 06 01 00 01 01 00 00 00 01 01 00 00 00 00 00 00 00 00 01 00 00 00 0c"),
            (Settings { free_mode: true, ..base }, "aa bb 17 06 01 01 01 01 00 00 00 01 01 00 00 00 00 00 00 00 00 00 00 00 00 0c"),
            (Settings { free_mode: true, mode: WorkMode::Cylinder, ..base }, "aa bb 17 06 01 01 01 01 00 00 01 01 01 00 00 00 00 00 00 00 00 00 00 00 00 0d"),
            (Settings { free_mode: true, mode: WorkMode::Slab, ..base }, "aa bb 17 06 01 01 01 01 00 00 01 00 01 00 00 00 00 00 00 00 00 00 00 00 00 0c"),
            (Settings { free_mode: true, mode: WorkMode::Trolley, ..base }, "aa bb 17 06 01 01 01 01 00 00 01 00 01 00 00 00 00 00 00 00 00 00 00 00 00 0c"),
        ];
        for (s, expected) in cases {
            assert_eq!(frame(&s.command()), hex(expected), "{s:?}");
        }
    }
}

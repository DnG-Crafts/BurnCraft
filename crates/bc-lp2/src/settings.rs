//! The machine's own settings (the LP2 app's "Mode setting" page), written with the `06 01`
//! command. The app writes them each time it connects and whenever one is changed; the machine
//! answers `06 01`.

use serde::{Deserialize, Serialize};

/// How the LP2 is set up for the work: flat, or one of the Versatile Electric Roller's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WorkMode {
    /// On its stand, over a flat surface (the roller off).
    #[default]
    Flat,
    /// Engraving round objects.
    Cylinder,
    Slab,
    Trolley,
}

impl WorkMode {
    pub const ALL: [WorkMode; 4] = [WorkMode::Flat, WorkMode::Cylinder, WorkMode::Slab, WorkMode::Trolley];
    /// The Versatile Electric Roller's modes.
    pub const ROLLER: [WorkMode; 3] = [WorkMode::Cylinder, WorkMode::Slab, WorkMode::Trolley];

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: WorkMode,
    /// The roller mode chosen last: turning the roller on goes back to it (cylinder at first).
    pub roller_mode: WorkMode,
    /// The preview light's power (1 is the LP2 app's own; higher is brighter, and can mark).
    pub preview_power: u8,
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
    /// The machine's own settings, as read when connecting (in the settings command's layout), so
    /// those BurnCraft doesn't show are written back as they were.
    #[serde(skip)]
    pub machine: Option<[u8; 21]>,
}

impl Default for Settings {
    /// As the LP2 app sets a new machine up.
    fn default() -> Self {
        Self { mode: WorkMode::Flat, roller_mode: WorkMode::Cylinder, preview_power: 1, free_mode: false, beep: true, path_optimization: true, reverse_direction: false, touch_buttons: true, batch_button: false, machine: None }
    }
}

impl Settings {
    /// Whether the Versatile Electric Roller is in use.
    pub fn roller(&self) -> bool {
        self.mode != WorkMode::Flat
    }

    /// The marking field (mm): 100 × 100, or 100 × 2000 with the roller.
    pub fn field(&self) -> (f64, f64) {
        if self.roller() { crate::ROLLER_FIELD } else { crate::FIELD }
    }

    /// Turns the roller on (in the roller mode chosen last) or off (flat surface), as the LP2 app
    /// does.
    pub fn set_roller(&mut self, on: bool) {
        self.mode = if on { self.roller_mode } else { WorkMode::Flat };
    }

    /// Chooses the roller mode (and turns the roller on).
    pub fn set_roller_mode(&mut self, mode: WorkMode) {
        if mode != WorkMode::Flat {
            self.roller_mode = mode;
            self.mode = mode;
        }
    }

    /// The machine's settings from its reply to the settings query, in the settings command's
    /// layout (as the LP2 app writes them back). The reply has the same fields one byte earlier,
    /// apart from two the command keeps at 0. None if the reply is too short.
    pub fn machine_command(reply: &[u8]) -> Option<[u8; 21]> {
        if reply.len() < 21 || reply[0] != 0x00 {
            return None;
        }
        let mut p = [0u8; 21];
        p[0] = 0x06;
        p[1] = 0x01;
        // Free mode, beep, view, g_view; then (after "custom") the roller flag and mode, touch
        // buttons, ir_dst, batch button, and the other accessory flags, up to the print direction.
        p[2..6].copy_from_slice(&reply[1..5]);
        p[7..19].copy_from_slice(&reply[7..19]);
        p[20] = reply[20];
        Some(p)
    }

    /// These settings with the machine's own (from [`Settings::machine_command`]): the switches
    /// BurnCraft shows take the machine's values, and the rest are kept to be written back.
    pub fn with_machine(&self, machine: [u8; 21]) -> Self {
        let on = |i: usize| machine[i] != 0;
        let mode = match (machine[7], machine[8]) {
            (0, _) => WorkMode::Flat,
            (_, 1) => WorkMode::Cylinder,
            // Slab and trolley send the same bytes: keep whichever was chosen last.
            _ if matches!(self.roller_mode, WorkMode::Slab | WorkMode::Trolley) => self.roller_mode,
            _ => WorkMode::Slab,
        };
        Self {
            mode,
            roller_mode: if mode == WorkMode::Flat { self.roller_mode } else { mode },
            free_mode: on(2),
            beep: on(3),
            path_optimization: on(4),
            touch_buttons: on(9),
            batch_button: on(11),
            reverse_direction: on(18),
            machine: Some(machine),
            ..*self
        }
    }

    /// The settings command (payload): the machine's own settings, with the switches BurnCraft
    /// shows set from these.
    pub fn command(&self) -> Vec<u8> {
        let mut p = self.machine.unwrap_or_default().to_vec();
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
    fn the_roller_remembers_its_last_mode() {
        let mut s = Settings::default();
        assert!(!s.roller());
        s.set_roller(true);
        assert_eq!(s.mode, WorkMode::Cylinder, "cylinder until another mode is chosen");
        s.set_roller_mode(WorkMode::Trolley);
        s.set_roller(false);
        assert_eq!(s.mode, WorkMode::Flat);
        s.set_roller(true);
        assert_eq!(s.mode, WorkMode::Trolley, "back to the mode chosen last");
        assert_eq!(s.field(), crate::ROLLER_FIELD, "the roller carries the work along Y");
        s.set_roller(false);
        assert_eq!(s.field(), crate::FIELD);
    }

    #[test]
    fn the_machines_own_settings_are_kept() {
        // The settings reply from the first capture, and what the app wrote back on connecting.
        let reply = hex("00 01 01 01 00 01 00 00 01 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 0a 02");
        let machine = Settings::machine_command(&reply).unwrap();
        assert_eq!(machine.to_vec(), hex("06 01 01 01 01 00 00 00 01 01 00 00 00 00 00 00 00 00 00 00 00"));
        let s = Settings::default().with_machine(machine);
        assert!(s.free_mode, "free mode was on in the machine");
        assert_eq!(s.mode, WorkMode::Flat);
        assert_eq!(s.command(), machine.to_vec(), "written back unchanged");
        // A setting BurnCraft doesn't show (here g_pwr, byte 15) is written back as it was.
        let mut other = machine;
        other[15] = 7;
        let mut s = Settings::default().with_machine(other);
        s.beep = false;
        let cmd = s.command();
        assert_eq!((cmd[3], cmd[15]), (0, 7));
        assert!(Settings::machine_command(&[0, 1, 2]).is_none(), "too short");
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

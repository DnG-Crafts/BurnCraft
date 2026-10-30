//! GRBL's own configuration (the `$$` settings): what each one means, reading them from the
//! controller's reply, and saving them to / loading them from a text file.
//!
//! Descriptions follow the GRBL 1.1 documentation of its settings. Controllers built on GRBL
//! (grblHAL, FluidNC and others) add settings of their own; those are shown by number.

/// How a setting's value is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A number.
    Number,
    /// 0 or 1.
    Switch,
    /// One bit per axis: X = 1, Y = 2, Z = 4.
    AxisMask,
}

/// What a setting is for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

const fn info(name: &'static str, unit: &'static str, kind: Kind, help: &'static str) -> Info {
    Info { name, unit, kind, help }
}

/// The meaning of setting `$id`, for the settings GRBL 1.1 defines.
pub fn info_for(id: u32) -> Option<Info> {
    use Kind::*;
    Some(match id {
        0 => info("Step pulse time", "µs", Number, "Length of each step pulse sent to the motor drivers."),
        1 => info("Step idle delay", "ms", Number, "How long the motors stay powered after a move. 255 keeps them powered, so the head can't be pushed by hand."),
        2 => info("Step pulse invert", "axes", AxisMask, "Inverts the step signal for the ticked axes."),
        3 => info("Direction invert", "axes", AxisMask, "Reverses the ticked axes. Use it if an axis moves the wrong way."),
        4 => info("Invert step enable", "", Switch, "Inverts the motor enable signal."),
        5 => info("Invert limit pins", "", Switch, "Inverts the limit switch inputs (for normally-closed switches)."),
        6 => info("Invert probe pin", "", Switch, "Inverts the probe input."),
        10 => info("Status report", "mask", Number, "What the controller includes in its position reports. 1 = machine position, 2 = buffer state."),
        11 => info("Junction deviation", "mm", Number, "How fast the head takes corners. Smaller is slower and more accurate."),
        12 => info("Arc tolerance", "mm", Number, "How closely arcs are followed."),
        13 => info("Report in inches", "", Switch, "Reports positions in inches instead of millimetres. Keep this off for BurnCraft."),
        20 => info("Soft limits", "", Switch, "Refuses moves past the travel set in $130–$132. Needs homing ($22) on."),
        21 => info("Hard limits", "", Switch, "Stops everything when a limit switch is hit during a job."),
        22 => info("Homing cycle", "", Switch, "Turns homing ($H) on. Only for machines with limit switches."),
        23 => info("Homing direction invert", "axes", AxisMask, "Homes the ticked axes towards the other end. Use it to choose which corner the machine homes to."),
        24 => info("Homing feed", "mm/min", Number, "Slow speed used to find the switch exactly."),
        25 => info("Homing seek", "mm/min", Number, "Fast speed used to find the switch first."),
        26 => info("Homing debounce", "ms", Number, "Delay that filters out switch bounce."),
        27 => info("Homing pull-off", "mm", Number, "How far the head backs off a switch after homing."),
        30 => info("Maximum spindle speed", "S", Number, "The S value meaning full laser power. BurnCraft's \"Full-power S value\" must match it."),
        31 => info("Minimum spindle speed", "S", Number, "The S value meaning the lowest power."),
        32 => info("Laser mode", "", Switch, "Laser mode: power follows speed, and the head doesn't stop between moves. Keep it on for lasers."),
        100 => info("X steps/mm", "steps/mm", Number, "Motor steps per millimetre of X travel. Wrong values make shapes the wrong size."),
        101 => info("Y steps/mm", "steps/mm", Number, "Motor steps per millimetre of Y travel."),
        102 => info("Z steps/mm", "steps/mm", Number, "Motor steps per millimetre of Z travel."),
        // A fourth (A) axis, on controllers that have one (grblHAL and others), often a rotary.
        103 => info("A steps/mm", "steps/mm", Number, "Motor steps per unit of the fourth (A) axis, often a rotary."),
        113 => info("A max rate", "mm/min", Number, "Fastest the A axis may move."),
        123 => info("A acceleration", "mm/s²", Number, "How quickly A speeds up and slows down."),
        133 => info("A max travel", "mm", Number, "Length of the A axis."),
        110 => info("X max rate", "mm/min", Number, "Fastest the X axis may move."),
        111 => info("Y max rate", "mm/min", Number, "Fastest the Y axis may move."),
        112 => info("Z max rate", "mm/min", Number, "Fastest the Z axis may move."),
        120 => info("X acceleration", "mm/s²", Number, "How quickly X speeds up and slows down."),
        121 => info("Y acceleration", "mm/s²", Number, "How quickly Y speeds up and slows down."),
        122 => info("Z acceleration", "mm/s²", Number, "How quickly Z speeds up and slows down."),
        130 => info("X max travel", "mm", Number, "Length of the X axis. Used for soft limits and homing."),
        131 => info("Y max travel", "mm", Number, "Length of the Y axis."),
        132 => info("Z max travel", "mm", Number, "Length of the Z axis."),
        _ => return None,
    })
}

/// One `$N=value` line, or `None` for any other line. Anything after the value (GRBL 0.9's
/// "(description)", or a `;` comment in a file) is ignored.
pub fn parse_line(line: &str) -> Option<(u32, String)> {
    let rest = line.trim().strip_prefix('$')?;
    let (id, value) = rest.split_once('=')?;
    let id: u32 = id.trim().parse().ok()?;
    let value = value.split(['(', ';']).next().unwrap_or("").trim();
    (!value.is_empty()).then(|| (id, value.to_string()))
}

/// Every setting in a reply to `$$`, or in a settings file, in order. Later lines for the same
/// setting replace earlier ones.
pub fn parse(text: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for (id, value) in text.lines().filter_map(parse_line) {
        match out.iter_mut().find(|(i, _)| *i == id) {
            Some(slot) => slot.1 = value,
            None => out.push((id, value)),
        }
    }
    out
}

/// A settings file: one `$N=value` line per setting, with what it is as a comment.
pub fn to_file(settings: &[(u32, String)], header: &str) -> String {
    let mut out = String::new();
    for line in header.lines() {
        out += &format!("; {line}\n");
    }
    for (id, value) in settings {
        let note = info_for(*id).map(|i| if i.unit.is_empty() || i.kind != Kind::Number { format!(" ; {}", i.name) } else { format!(" ; {} ({})", i.name, i.unit) }).unwrap_or_default();
        out += &format!("${id}={value}{note}\n");
    }
    out
}

/// Whether `value` suits setting `id`: a number, 0/1 for switches, 0–7 for axis masks.
pub fn check(id: u32, value: &str) -> Result<(), String> {
    let v: f64 = value.trim().parse().map_err(|_| format!("${id}: \"{value}\" isn't a number"))?;
    match info_for(id).map(|i| i.kind) {
        Some(Kind::Switch) if v != 0.0 && v != 1.0 => Err(format!("${id} must be 0 or 1")),
        Some(Kind::AxisMask) if !(0.0..=7.0).contains(&v) || v.fract() != 0.0 => Err(format!("${id} must be a whole number from 0 to 7")),
        _ if v < 0.0 => Err(format!("${id} can't be negative")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_controller_replies_and_files() {
        let reply = "$0=10\r\n$1=25\r\n$22=0\r\n$110=6000.000\r\n$130=400 (x max travel, mm)\r\nok\r\n";
        let s = parse(reply);
        assert_eq!(s, vec![(0, "10".into()), (1, "25".into()), (22, "0".into()), (110, "6000.000".into()), (130, "400".into())]);
        assert_eq!(parse_line("[MSG:'$H'|'$X' to unlock]"), None);
        assert_eq!(parse_line("$N0="), None);
        assert_eq!(parse_line("$32=1 ; laser mode"), Some((32, "1".into())));
    }

    #[test]
    fn files_round_trip() {
        let s = vec![(22, "1".to_string()), (110, "6000".to_string()), (400, "3".to_string())];
        let file = to_file(&s, "NEJE 4 Max\nsaved by BurnCraft");
        assert!(file.starts_with("; NEJE 4 Max\n"));
        assert!(file.contains("$110=6000 ; X max rate (mm/min)"));
        assert!(file.contains("$400=3\n"), "unknown settings kept, without a note");
        assert_eq!(parse(&file), s);
    }

    #[test]
    fn values_are_checked() {
        assert!(check(22, "1").is_ok());
        assert!(check(22, "2").is_err());
        assert!(check(3, "5").is_ok());
        assert!(check(3, "9").is_err());
        assert!(check(110, "abc").is_err());
        assert!(check(110, "-5").is_err());
        assert!(check(110, "6000.5").is_ok());
        assert!(info_for(32).is_some_and(|i| i.kind == Kind::Switch));
    }
}

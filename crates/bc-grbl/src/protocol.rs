//! Parsing GRBL's responses.

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// "Idle", "Run", "Hold:0", "Jog", "Alarm", "Door:0", "Home", ...
    pub state: String,
    /// Machine position, if reported.
    pub mpos: Option<(f64, f64, f64)>,
    /// Work position, if reported.
    pub wpos: Option<(f64, f64, f64)>,
    /// Work coordinate offset, if reported.
    pub wco: Option<(f64, f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Ok,
    Error(u32),
    Alarm(u32),
    Status(Status),
    /// Startup banner, e.g. "Grbl 1.1h ['$' for help]".
    Welcome(String),
    /// Anything else ([MSG:...], settings, feedback).
    Other(String),
}

fn xyz(s: &str) -> Option<(f64, f64, f64)> {
    let mut it = s.split(',').map(|v| v.trim().parse::<f64>());
    let x = it.next()?.ok()?;
    let y = it.next()?.ok()?;
    let z = it.next().and_then(Result::ok).unwrap_or(0.0);
    Some((x, y, z))
}

pub fn parse_line(line: &str) -> Response {
    let line = line.trim();
    if line == "ok" {
        return Response::Ok;
    }
    if let Some(n) = line.strip_prefix("error:") {
        return Response::Error(n.trim().parse().unwrap_or(0));
    }
    if let Some(n) = line.strip_prefix("ALARM:") {
        return Response::Alarm(n.trim().parse().unwrap_or(0));
    }
    if line.starts_with("Grbl ") || line.starts_with("GrblHAL") {
        return Response::Welcome(line.to_string());
    }
    if let Some(body) = line.strip_prefix('<').and_then(|l| l.strip_suffix('>')) {
        let mut fields = body.split('|');
        let state = fields.next().unwrap_or_default().to_string();
        let mut status = Status { state, mpos: None, wpos: None, wco: None };
        for f in fields {
            if let Some(v) = f.strip_prefix("MPos:") {
                status.mpos = xyz(v);
            } else if let Some(v) = f.strip_prefix("WPos:") {
                status.wpos = xyz(v);
            } else if let Some(v) = f.strip_prefix("WCO:") {
                status.wco = xyz(v);
            }
        }
        return Response::Status(status);
    }
    Response::Other(line.to_string())
}

/// Human-readable meaning of common GRBL alarm codes.
pub fn alarm_text(code: u32) -> &'static str {
    match code {
        1 => "hard limit triggered; re-home the machine",
        2 => "motion target exceeds the machine travel (soft limit)",
        3 => "reset while moving; position may be lost, re-home",
        4 | 5 => "probe fail",
        6..=9 => "homing failed",
        _ => "alarm",
    }
}

/// What a GRBL 1.1 `error:N` reply means, in plain words.
pub fn error_text(code: u32) -> &'static str {
    match code {
        1 => "a G-code word has a number but no letter",
        2 => "a number is missing or badly written",
        3 => "the controller doesn't know this $ command",
        4 => "a value that must be positive is negative",
        5 => "homing is turned off in the controller's settings ($22)",
        6 => "the step pulse setting is too short",
        7 => "the controller's settings memory couldn't be read; settings were reset to defaults",
        8 => "this $ command only works while the machine is idle",
        9 => "the machine is in an alarm or jogging; unlock or home it first",
        10 => "soft limits need homing to be turned on",
        11 => "the line is too long",
        12 => "the setting would need a faster step rate than the controller supports",
        13 => "the safety door is open",
        14 => "the text is too long to store",
        15 => "the move would go past the machine's travel",
        16 => "the jog command isn't valid",
        17 => "laser mode needs the PWM output enabled",
        20 => "the controller doesn't support this G-code command",
        21 => "two commands of the same kind are on one line",
        22 => "no feed rate (speed) has been set",
        23 => "the command needs a whole number",
        24 => "two commands on one line both need the axis values",
        25 => "a G-code word is repeated on the line",
        26 => "the command needs X, Y or Z values but has none",
        27 => "the line number is out of range",
        28 => "the command is missing a P or L value",
        29 => "that work coordinate system isn't supported",
        30 => "G53 needs G0 or G1 to be active",
        31 => "there are axis values with no move to use them",
        32 => "the arc has no end point in the chosen plane",
        33 => "the move's target isn't valid",
        34 => "the arc's radius doesn't fit its end points",
        35 => "the arc is missing its centre offset",
        36 => "the line has words no command uses",
        37 => "the tool length offset can only apply to its own axis",
        38 => "the tool number is too high",
        _ => "the controller reported an error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_responses() {
        assert_eq!(parse_line("ok\r"), Response::Ok);
        assert_eq!(parse_line("error:20"), Response::Error(20));
        assert_eq!(parse_line("ALARM:2"), Response::Alarm(2));
        assert!(error_text(5).contains("$22"));
        assert_eq!(error_text(99), "the controller reported an error");
        assert!(matches!(parse_line("Grbl 1.1h ['$' for help]"), Response::Welcome(_)));
        let Response::Status(s) = parse_line("<Idle|MPos:1.000,2.500,0.000|FS:0,0|WCO:0.000,0.500,0.000>") else { panic!() };
        assert_eq!(s.state, "Idle");
        assert_eq!(s.mpos, Some((1.0, 2.5, 0.0)));
        assert_eq!(s.wco, Some((0.0, 0.5, 0.0)));
        let Response::Status(s) = parse_line("<Run|WPos:3,4|FS:1200,500>") else { panic!() };
        assert_eq!(s.wpos, Some((3.0, 4.0, 0.0)));
    }
}

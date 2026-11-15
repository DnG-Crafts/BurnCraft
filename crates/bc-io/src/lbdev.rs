//! LightBurn device files (`.lbdev`): JSON with a `DeviceList` of machine profiles.
//!
//! Laser makers publish these to set LightBurn up for their machines (e.g. LaserPecker).
//! We read the machine settings BurnCraft can use; UI and simulation preferences are ignored.

use serde_json::Value;

/// A machine profile from a device file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LbDevice {
    pub name: String,
    /// LightBurn's driver name, e.g. "GRBL", "GRBL-M3", "Ruida", "Smoothieware", "Marlin".
    pub driver: String,
    /// "Serial", "Ethernet", "USB", …
    pub connection: String,
    /// Work area in mm.
    pub width: f64,
    pub height: f64,
    /// LightBurn's origin flags: neither = front-left, Y = rear-left, X = front-right, both = rear-right.
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub home_on_startup: bool,
    pub port: Option<String>,
    pub baud: Option<u32>,
    pub address: Option<String>,
    /// S value for full power.
    pub s_max: Option<f64>,
    pub start_gcode: String,
    pub end_gcode: String,
    pub air_m7: bool,
    pub dtr: Option<bool>,
    /// (label, G-code) buttons.
    pub macros: Vec<(String, String)>,
}

pub const EXTENSION: &str = "lbdev";

pub fn parse(data: &[u8]) -> Result<Vec<LbDevice>, String> {
    let root: Value = serde_json::from_slice(data).map_err(|e| format!("not a LightBurn device file: {e}"))?;
    let list = root.get("DeviceList").and_then(Value::as_array).ok_or("not a LightBurn device file (no DeviceList)")?;
    let devices: Vec<LbDevice> = list.iter().map(device).collect();
    if devices.is_empty() {
        return Err("the file contains no devices".into());
    }
    Ok(devices)
}

fn device(d: &Value) -> LbDevice {
    let s = d.get("Settings").cloned().unwrap_or(Value::Null);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string).filter(|t| !t.trim().is_empty());
    let number = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool);
    let mut macros = Vec::new();
    for i in 0.. {
        let (Some(label), content) = (s.get(format!("Macro{i}_Label")), s.get(format!("Macro{i}_Content"))) else { break };
        let label = label.as_str().unwrap_or("").trim().to_string();
        let content = content.and_then(Value::as_str).unwrap_or("").trim().to_string();
        if !content.is_empty() {
            macros.push((if label.is_empty() || label == "none" { content.lines().next().unwrap_or("").to_string() } else { label }, content));
        }
    }
    // "Info" holds the connection target: the IP address of a network (UDP/Ethernet) device, and
    // sometimes the port of a serial one.
    let info = text(d, "Info").map(|i| i.trim().to_string());
    let info_ip = info.clone().filter(|i| i.parse::<std::net::Ipv4Addr>().is_ok());
    let info_port = info.filter(|i| {
        let upper = i.to_ascii_uppercase();
        (upper.starts_with("COM") && upper[3..].parse::<u32>().is_ok()) || i.starts_with("/dev/")
    });
    LbDevice {
        name: text(d, "DisplayName").or_else(|| text(d, "Name")).unwrap_or_else(|| "Imported laser".into()),
        driver: text(d, "Name").or_else(|| text(d, "ProfilePath")).unwrap_or_default(),
        connection: text(d, "Type").unwrap_or_default(),
        width: number(d, "Width").unwrap_or(0.0),
        height: number(d, "Height").unwrap_or(0.0),
        mirror_x: flag(d, "MirrorX").unwrap_or(false),
        mirror_y: flag(d, "MirrorY").unwrap_or(false),
        home_on_startup: flag(d, "HomeOnStartup").unwrap_or(false),
        port: text(&s, "CommPort").or(info_port),
        baud: number(&s, "BaudRate").map(|b| b as u32).filter(|b| *b > 0),
        address: text(d, "Address").or_else(|| text(&s, "Address")).or_else(|| text(&s, "IPAddress")).or(info_ip),
        s_max: number(&s, "S_Scale").filter(|v| *v > 0.0),
        start_gcode: text(&s, "StartGCode").unwrap_or_default(),
        end_gcode: text(&s, "EndGCode").unwrap_or_default(),
        air_m7: flag(&s, "AirAssistM7").unwrap_or(false),
        dtr: flag(&s, "EnableDTR"),
        macros,
    }
}

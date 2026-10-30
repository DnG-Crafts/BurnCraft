//! Variable text: templates such as `Part {serial:0000}`, `{date}` or `{Name}` (a CSV column),
//! filled in when a job is built.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VariableText {
    /// Current serial number.
    pub serial: i64,
    /// Added to the serial number after each job (when `auto_advance` is on).
    pub increment: i64,
    /// Advance the serial number and CSV row after each job sent.
    pub auto_advance: bool,
    /// Column names of the loaded CSV file.
    pub csv_headers: Vec<String>,
    pub csv_rows: Vec<Vec<String>>,
    /// Current CSV row (0-based).
    pub row: usize,
}

impl Default for VariableText {
    fn default() -> Self {
        Self { serial: 1, increment: 1, auto_advance: true, csv_headers: Vec::new(), csv_rows: Vec::new(), row: 0 }
    }
}

/// Values for `{date}` and `{time}`, supplied by the caller (so this crate needs no clock).
#[derive(Debug, Clone, Default)]
pub struct Clock {
    pub date: String,
    pub time: String,
}

/// Whether `template` contains any `{...}` field.
pub fn has_fields(template: &str) -> bool {
    template.find('{').is_some_and(|i| template[i..].contains('}'))
}

impl VariableText {
    /// Moves to the next serial number and CSV row.
    pub fn advance(&mut self) {
        self.serial += self.increment;
        if !self.csv_rows.is_empty() {
            self.row = (self.row + 1).min(self.csv_rows.len() - 1);
        }
    }

    fn csv_value(&self, column: &str) -> Option<&str> {
        let col = self.csv_headers.iter().position(|h| h.eq_ignore_ascii_case(column.trim()))?;
        self.csv_rows.get(self.row)?.get(col).map(String::as_str)
    }

    /// Fills in a template. Fields: `{serial}`, `{serial:000}` (zero-padded to the pattern's width),
    /// `{date}`, `{time}`, `{row}` (1-based CSV row), and `{ColumnName}` for the current CSV row.
    /// Unknown fields are left as written; `{{` and `}}` produce literal braces.
    pub fn evaluate(&self, template: &str, clock: &Clock) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(i) = rest.find(['{', '}']) {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            if tail.starts_with("{{") || tail.starts_with("}}") {
                out.push(tail.as_bytes()[0] as char);
                rest = &tail[2..];
                continue;
            }
            if let Some(after) = tail.strip_prefix('}') {
                out.push('}');
                rest = after;
                continue;
            }
            let Some(end) = tail.find('}') else {
                out.push_str(tail);
                rest = "";
                break;
            };
            let field = &tail[1..end];
            let (name, format) = field.split_once(':').unwrap_or((field, ""));
            let value = match name.trim().to_ascii_lowercase().as_str() {
                "serial" => Some(if format.is_empty() { self.serial.to_string() } else { format!("{:0width$}", self.serial, width = format.len()) }),
                "date" => Some(clock.date.clone()),
                "time" => Some(clock.time.clone()),
                "row" => Some((self.row + 1).to_string()),
                _ => self.csv_value(name).map(str::to_string),
            };
            match value {
                Some(v) => out.push_str(&v),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> VariableText {
        VariableText { serial: 7, csv_headers: vec!["Name".into(), "Size".into()], csv_rows: vec![vec!["Ada".into(), "M".into()], vec!["Bob".into(), "L".into()]], ..Default::default() }
    }

    #[test]
    fn fields() {
        let clock = Clock { date: "2026-10-14".into(), time: "09:30".into() };
        let v = vars();
        assert_eq!(v.evaluate("No. {serial:0000} for {name} ({Size}) on {date} {time}", &clock), "No. 0007 for Ada (M) on 2026-10-14 09:30");
        assert_eq!(v.evaluate("{{literal}} {unknown} {serial}", &clock), "{literal} {unknown} 7");
        assert_eq!(v.evaluate("unterminated {serial", &clock), "unterminated {serial");
    }

    #[test]
    fn advancing() {
        let mut v = vars();
        v.increment = 5;
        v.advance();
        assert_eq!((v.serial, v.row), (12, 1));
        v.advance();
        assert_eq!(v.row, 1, "stays on the last row");
        assert!(has_fields("a {b} c") && !has_fields("plain"));
    }
}

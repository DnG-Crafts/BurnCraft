//! Parametric design: named values (`thickness = 3`) and locked sizes and distances whose values
//! can be formulas (`thickness * 2 + 0.2`). Changing a value updates the design.
//!
//! A constraint acts on a shape's whole group, as selecting does: a locked width scales the group
//! about the chosen edge, and a locked distance moves the second group so the gap is right.

use crate::Document;
use bc_geom::{Affine, Rect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
}

/// A point along one axis of a bounding box: left/top, centre, or right/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

impl Anchor {
    pub const ALL: [Anchor; 3] = [Anchor::Start, Anchor::Middle, Anchor::End];

    pub fn label(self, axis: Axis) -> &'static str {
        match (self, axis) {
            (Anchor::Start, Axis::X) => "left",
            (Anchor::Start, Axis::Y) => "top",
            (Anchor::Middle, Axis::X) => "centre",
            (Anchor::Middle, Axis::Y) => "middle",
            (Anchor::End, Axis::X) => "right",
            (Anchor::End, Axis::Y) => "bottom",
        }
    }

    fn of(self, r: Rect, axis: Axis) -> f64 {
        let (lo, hi) = match axis {
            Axis::X => (r.x0, r.x1),
            Axis::Y => (r.y0, r.y1),
        };
        match self {
            Anchor::Start => lo,
            Anchor::Middle => (lo + hi) / 2.0,
            Anchor::End => hi,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConstraintKind {
    /// The width (X) or height (Y) of a shape's group; `anchor` is the edge that stays put.
    Size { shape: u64, axis: Axis, anchor: Anchor },
    /// The distance along an axis from a point of one group to a point of another; the second
    /// group moves.
    Distance { axis: Axis, from: u64, from_anchor: Anchor, to: u64, to_anchor: Anchor },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    /// A number or formula (mm), e.g. `40` or `thickness * 2`.
    pub value: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    /// A number or formula using earlier parameters.
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Parametric {
    pub params: Vec<Param>,
    pub constraints: Vec<Constraint>,
}

impl Parametric {
    pub fn is_empty(&self) -> bool {
        self.params.is_empty() && self.constraints.is_empty()
    }

    /// Every parameter's value, in order (each may use the ones before it). Errors name the parameter.
    pub fn values(&self) -> (HashMap<String, f64>, Vec<String>) {
        let mut vars = HashMap::new();
        let mut errors = Vec::new();
        for p in &self.params {
            match eval(&p.value, &vars) {
                Ok(v) => {
                    vars.insert(p.name.trim().to_string(), v);
                }
                Err(e) => errors.push(format!("{}: {e}", p.name)),
            }
        }
        (vars, errors)
    }

    /// A name not used by any parameter yet.
    pub fn new_name(&self) -> String {
        (1..).map(|i| format!("p{i}")).find(|n| !self.params.iter().any(|p| &p.name == n)).expect("some name is free")
    }
}

impl Constraint {
    /// The shapes this constraint refers to.
    pub fn shapes(&self) -> Vec<u64> {
        match self.kind {
            ConstraintKind::Size { shape, .. } => vec![shape],
            ConstraintKind::Distance { from, to, .. } => vec![from, to],
        }
    }

    /// The constrained quantity as the design is now.
    pub fn measure(&self, doc: &Document) -> Option<f64> {
        match self.kind {
            ConstraintKind::Size { shape, axis, .. } => {
                let r = doc.bounds_of(unit(doc, shape))?;
                Some(match axis {
                    Axis::X => r.width(),
                    Axis::Y => r.height(),
                })
            }
            ConstraintKind::Distance { axis, from, from_anchor, to, to_anchor } => {
                let (a, b) = (doc.bounds_of(unit(doc, from))?, doc.bounds_of(unit(doc, to))?);
                Some(to_anchor.of(b, axis) - from_anchor.of(a, axis))
            }
        }
    }

    pub fn describe(&self, doc: &Document) -> String {
        let name = |id: u64| doc.shape(id).map_or_else(|| "(deleted)".to_string(), |s| format!("{} #{id}", s.kind.type_name()));
        match &self.kind {
            ConstraintKind::Size { shape, axis, anchor } => {
                format!("{} of {} (keeps its {})", if *axis == Axis::X { "Width" } else { "Height" }, name(*shape), anchor.label(*axis))
            }
            ConstraintKind::Distance { axis, from, from_anchor, to, to_anchor } => format!(
                "{} from {} {} to {} {}",
                if *axis == Axis::X { "Across" } else { "Down" },
                name(*from),
                from_anchor.label(*axis),
                name(*to),
                to_anchor.label(*axis)
            ),
        }
    }
}

/// The ids that move together with `id`: its group, or just itself.
pub fn unit(doc: &Document, id: u64) -> Vec<u64> {
    match doc.shape(id).and_then(|s| s.group) {
        Some(g) => doc.shapes.iter().filter(|s| s.group == Some(g)).map(|s| s.id).collect(),
        None => vec![id],
    }
}

/// Makes the design meet its constraints. Returns whether anything moved or changed size, and
/// messages about constraints that can't be met. Constraints on deleted shapes are dropped.
pub fn solve(doc: &mut Document) -> (bool, Vec<String>) {
    let present: std::collections::HashSet<u64> = doc.shapes.iter().map(|s| s.id).collect();
    doc.params.constraints.retain(|c| c.shapes().iter().all(|id| present.contains(id)));
    if doc.params.constraints.is_empty() {
        return (false, Vec::new());
    }
    let (vars, mut errors) = doc.params.values();
    let targets: Vec<Option<f64>> = doc
        .params
        .constraints
        .iter()
        .map(|c| {
            if !c.enabled {
                return None;
            }
            match eval(&c.value, &vars) {
                Ok(v) => Some(v),
                Err(e) => {
                    errors.push(format!("{}: {e}", c.describe(doc)));
                    None
                }
            }
        })
        .collect();
    let mut changed = false;
    // Constraints can depend on each other (a width moves an edge another distance is measured
    // from), so go round until nothing moves.
    for _ in 0..20 {
        let mut moved = false;
        for (c, target) in doc.params.constraints.clone().iter().zip(&targets) {
            let (Some(v), Some(now)) = (*target, c.measure(doc)) else { continue };
            if (now - v).abs() < 1e-6 {
                continue;
            }
            let (ids, t) = match c.kind {
                ConstraintKind::Size { shape, axis, anchor } => {
                    if v <= 0.0 || now <= 1e-9 {
                        continue;
                    }
                    let ids = unit(doc, shape);
                    let Some(r) = doc.bounds_of(ids.iter().copied()) else { continue };
                    let k = v / now;
                    let pivot = anchor.of(r, axis);
                    let t = match axis {
                        Axis::X => Affine::translate((pivot, 0.0)) * Affine::scale_non_uniform(k, 1.0) * Affine::translate((-pivot, 0.0)),
                        Axis::Y => Affine::translate((0.0, pivot)) * Affine::scale_non_uniform(1.0, k) * Affine::translate((0.0, -pivot)),
                    };
                    (ids, t)
                }
                ConstraintKind::Distance { axis, to, .. } => {
                    let d = v - now;
                    (unit(doc, to), if axis == Axis::X { Affine::translate((d, 0.0)) } else { Affine::translate((0.0, d)) })
                }
            };
            if ids.iter().any(|id| doc.shape(*id).is_some_and(|s| s.locked)) {
                let msg = format!("{}: the shape is locked", c.describe(doc));
                if !errors.contains(&msg) {
                    errors.push(msg);
                }
                continue;
            }
            for s in doc.shapes.iter_mut().filter(|s| ids.contains(&s.id)) {
                s.transform = t * s.transform;
            }
            moved = true;
            changed = true;
        }
        if !moved {
            return (changed, errors);
        }
    }
    errors.push("Some constraints fight each other and can't all be met".into());
    (changed, errors)
}

// --- Formulas -----------------------------------------------------------------

/// Evaluates a formula: numbers, parameter names, + − × ÷ (`*` `/`), brackets and unary minus.
pub fn eval(text: &str, vars: &HashMap<String, f64>) -> Result<f64, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0, vars };
    let v = p.expr()?;
    p.space();
    if p.i < p.s.len() {
        return Err(format!("unexpected \"{}\"", &text[p.i..]));
    }
    if !v.is_finite() {
        return Err("the result isn't a number (dividing by zero?)".into());
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    vars: &'a HashMap<String, f64>,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.i += 1;
            let r = self.term()?;
            v = if op == b'+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.factor()?;
        while let Some(op @ (b'*' | b'/' | b'x')) = self.peek() {
            // "x" only multiplies between numbers, like "2 x 3"; otherwise it starts a name.
            if op == b'x' && self.s.get(self.i + 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                break;
            }
            self.i += 1;
            let r = self.factor()?;
            v = if op == b'/' { v / r } else { v * r };
        }
        Ok(v)
    }

    fn factor(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(-self.factor()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.factor()
            }
            Some(b'(') => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err("missing \")\"".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).expect("ASCII");
                t.parse().map_err(|_| format!("\"{t}\" isn't a number"))
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
                    self.i += 1;
                }
                let name = std::str::from_utf8(&self.s[start..self.i]).expect("ASCII");
                self.vars.get(name).copied().ok_or_else(|| format!("no value called \"{name}\""))
            }
            Some(_) => Err(format!("unexpected \"{}\"", String::from_utf8_lossy(&self.s[self.i..]))),
            None => Err("a value is missing".into()),
        }
    }
}

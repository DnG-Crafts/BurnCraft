//! More part generators: gears, sign plates and a library of ready-made shapes.

use crate::{ellipse_path, polyline_path, rect_path, Affine, BezPath, Point};
use std::f64::consts::{PI, TAU};

// --- Gears ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearSpec {
    pub teeth: u32,
    /// Module: pitch diameter ÷ teeth (mm). Gears that mesh share a module.
    pub module: f64,
    /// Pressure angle (degrees); 20° is standard.
    pub pressure_angle: f64,
    /// Centre hole diameter (mm); 0 for none.
    pub bore: f64,
    /// Width of a keyway in the bore (mm); 0 for none.
    pub keyway: f64,
}

impl GearSpec {
    pub fn pitch_diameter(&self) -> f64 {
        self.module * self.teeth as f64
    }

    pub fn outside_diameter(&self) -> f64 {
        self.module * (self.teeth as f64 + 2.0)
    }
}

/// An involute spur gear centred on the origin (the outline plus the bore, if any).
pub fn gear(spec: &GearSpec) -> BezPath {
    let n = spec.teeth.max(4) as f64;
    let m = spec.module.max(0.1);
    let alpha = spec.pressure_angle.clamp(10.0, 35.0).to_radians();
    let r = m * n / 2.0;
    let rb = r * alpha.cos();
    let ra = r + m;
    let rf = (r - 1.25 * m).max(m * 0.5);
    let inv = |a: f64| a.tan() - a;
    // Polar angle of the involute (from the base circle) at radius `rad`.
    let involute_angle = |rad: f64| {
        let a = (rb / rad.max(rb)).acos();
        inv(a)
    };
    // Half the tooth's angular width at the pitch circle, and where its lower flank starts.
    let half = PI / (2.0 * n);
    let start = -half - inv(alpha);
    const FLANK: usize = 12;
    let polar = |rad: f64, angle: f64| Point::new(rad * angle.cos(), rad * angle.sin());
    let mut pts: Vec<Point> = Vec::new();
    for k in 0..n as usize {
        let base = k as f64 * TAU / n;
        let r0 = rb.max(rf);
        // Lower flank, root to tip.
        if rf < rb {
            pts.push(polar(rf, base + start));
        }
        for i in 0..=FLANK {
            let rad = r0 + (ra - r0) * i as f64 / FLANK as f64;
            pts.push(polar(rad, base + start + involute_angle(rad)));
        }
        // Upper flank (mirror of the lower one about the tooth's centre line), tip to root.
        for i in (0..=FLANK).rev() {
            let rad = r0 + (ra - r0) * i as f64 / FLANK as f64;
            pts.push(polar(rad, base - start - involute_angle(rad)));
        }
        if rf < rb {
            pts.push(polar(rf, base - start));
        }
    }
    let mut out = polyline_path(&pts, true);
    if spec.bore > 0.0 && spec.bore < 2.0 * rf {
        let br = spec.bore / 2.0;
        if spec.keyway > 0.0 && spec.keyway < spec.bore {
            // Round bore with a square keyway notch at the top.
            let hw = spec.keyway / 2.0;
            let depth = br + spec.keyway / 2.0;
            let a0 = (hw / br).asin();
            let mut hole: Vec<Point> = Vec::new();
            hole.push(Point::new(hw, -depth));
            for i in 0..=48 {
                let a = -PI / 2.0 + a0 + (TAU - 2.0 * a0) * i as f64 / 48.0;
                hole.push(polar(br, a));
            }
            hole.push(Point::new(-hw, -depth));
            out.extend(polyline_path(&hole, true));
        } else {
            out.extend(ellipse_path(br, br));
        }
    }
    out
}

// --- Signs --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignShape {
    Rectangle,
    Rounded,
    Oval,
    /// A rectangle with its corners cut inward (a classic plaque).
    Plaque,
}

impl SignShape {
    pub const ALL: [SignShape; 4] = [SignShape::Rectangle, SignShape::Rounded, SignShape::Oval, SignShape::Plaque];

    pub fn label(self) -> &'static str {
        match self {
            SignShape::Rectangle => "Rectangle",
            SignShape::Rounded => "Rounded",
            SignShape::Oval => "Oval",
            SignShape::Plaque => "Plaque",
        }
    }
}

/// A sign's outline (centred on the origin) at `width` × `height`; `corner` sizes the rounding or
/// the plaque's cut corners.
pub fn sign_outline(shape: SignShape, width: f64, height: f64, corner: f64) -> BezPath {
    let c = corner.clamp(0.0, width.min(height) / 2.0);
    match shape {
        SignShape::Rectangle => rect_path(width, height, 0.0),
        SignShape::Rounded => rect_path(width, height, c),
        SignShape::Oval => ellipse_path(width / 2.0, height / 2.0),
        SignShape::Plaque => {
            let (w, h) = (width / 2.0, height / 2.0);
            let mut p = BezPath::new();
            // Each corner is a quarter circle bitten out of the rectangle.
            p.move_to((-w + c, -h));
            p.line_to((w - c, -h));
            p.quad_to((w - c, -h + c), (w, -h + c));
            p.line_to((w, h - c));
            p.quad_to((w - c, h - c), (w - c, h));
            p.line_to((-w + c, h));
            p.quad_to((-w + c, h - c), (-w, h - c));
            p.line_to((-w, -h + c));
            p.quad_to((-w + c, -h + c), (-w + c, -h));
            p.close_path();
            p
        }
    }
}

/// Mounting holes: `count` (2 or 4) holes of `diameter`, `inset` mm in from the edges of a
/// `width` × `height` sign centred on the origin. Two holes sit at the top corners... unless the
/// sign is an oval, where they sit left and right of centre.
pub fn mounting_holes(count: u32, width: f64, height: f64, diameter: f64, inset: f64, oval: bool) -> BezPath {
    let r = diameter / 2.0;
    let (x, y) = (width / 2.0 - inset - r, height / 2.0 - inset - r);
    let centres: Vec<(f64, f64)> = match (count, oval) {
        (0, _) => vec![],
        (_, true) => vec![(-x, 0.0), (x, 0.0)],
        (1, _) => vec![(0.0, -y)],
        (2 | 3, _) => vec![(-x, -y), (x, -y)],
        _ => vec![(-x, -y), (x, -y), (x, y), (-x, y)],
    };
    let mut out = BezPath::new();
    for (cx, cy) in centres {
        out.extend(Affine::translate((cx, cy)) * ellipse_path(r, r));
    }
    out
}

/// An edge-lit stand for a sign `width` wide cut from material `thickness` thick: the sign gets
/// a tab (returned first, to add under the sign at y = 0 going down, centred) and the base plate
/// a matching slot. Returns (tab outline, base plate with slot), both centred horizontally.
pub fn edge_lit_stand(width: f64, thickness: f64) -> (BezPath, BezPath) {
    let tab_w = width * 0.7;
    let tab_h = thickness * 2.0;
    let tab = Affine::translate((0.0, tab_h / 2.0)) * rect_path(tab_w, tab_h, 0.0);
    let base_w = tab_w + 30.0;
    let base_h = (thickness * 2.0 + 30.0).max(40.0);
    let mut base = rect_path(base_w, base_h, 4.0);
    base.extend(rect_path(tab_w + 0.2, thickness + 0.1, 0.0));
    (tab, base)
}

// --- Shape library ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryShape {
    Heart,
    Arrow,
    SpeechBubble,
    Badge,
    Banner,
    Starburst,
    Cloud,
    Shield,
    Tag,
    House,
}

impl LibraryShape {
    pub const ALL: [LibraryShape; 10] = [
        LibraryShape::Heart,
        LibraryShape::Arrow,
        LibraryShape::SpeechBubble,
        LibraryShape::Badge,
        LibraryShape::Banner,
        LibraryShape::Starburst,
        LibraryShape::Cloud,
        LibraryShape::Shield,
        LibraryShape::Tag,
        LibraryShape::House,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LibraryShape::Heart => "Heart",
            LibraryShape::Arrow => "Arrow",
            LibraryShape::SpeechBubble => "Speech bubble",
            LibraryShape::Badge => "Badge",
            LibraryShape::Banner => "Banner",
            LibraryShape::Starburst => "Starburst",
            LibraryShape::Cloud => "Cloud",
            LibraryShape::Shield => "Shield",
            LibraryShape::Tag => "Gift tag",
            LibraryShape::House => "House",
        }
    }

    /// The shape, about `size` mm across, centred on the origin.
    pub fn path(self, size: f64) -> BezPath {
        let unit = self.unit();
        let b = crate::bounds(&unit).expect("library shapes are not empty");
        let s = size / b.width().max(b.height());
        Affine::scale(s) * Affine::translate(-b.center().to_vec2()) * unit
    }

    /// The shape at an arbitrary size.
    fn unit(self) -> BezPath {
        let svg = match self {
            LibraryShape::Heart => "M50,90 C20,70 0,50 0,28 C0,10 14,0 27,0 C38,0 46,7 50,16 C54,7 62,0 73,0 C86,0 100,10 100,28 C100,50 80,70 50,90 Z",
            LibraryShape::Arrow => "M0,35 L60,35 L60,10 L100,50 L60,90 L60,65 L0,65 Z",
            LibraryShape::SpeechBubble => "M15,0 L85,0 C93,0 100,7 100,15 L100,55 C100,63 93,70 85,70 L45,70 L22,92 L27,70 L15,70 C7,70 0,63 0,55 L0,15 C0,7 7,0 15,0 Z",
            LibraryShape::Banner => "M0,20 L15,20 L15,10 L85,10 L85,20 L100,20 L90,37 L100,54 L80,54 L80,44 L20,44 L20,54 L0,54 L10,37 Z M15,20 L15,44 L20,44 M85,20 L85,44 L80,44",
            LibraryShape::Cloud => "M25,70 C10,70 0,60 0,48 C0,36 10,27 22,28 C24,14 36,4 50,4 C63,4 73,12 76,24 C90,24 100,34 100,47 C100,60 90,70 77,70 Z",
            LibraryShape::Shield => "M50,0 C62,8 80,10 95,10 L95,45 C95,72 75,88 50,100 C25,88 5,72 5,45 L5,10 C20,10 38,8 50,0 Z",
            LibraryShape::Tag => "M25,0 L100,0 L100,50 L25,50 L0,25 Z M18,21 A4,4 0 1 0 18,29 A4,4 0 1 0 18,21 Z",
            LibraryShape::House => "M50,0 L100,45 L88,45 L88,100 L12,100 L12,45 L0,45 Z",
            LibraryShape::Badge => return scalloped(20, 50.0, 6.0),
            LibraryShape::Starburst => return starburst(24, 50.0, 40.0),
        };
        BezPath::from_svg(svg).expect("built-in shape data parses")
    }
}

/// A circle with `bumps` rounded scallops around its edge.
fn scalloped(bumps: u32, radius: f64, depth: f64) -> BezPath {
    let n = bumps.max(3) as usize * 16;
    let pts: Vec<Point> = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * TAU;
            let r = radius - depth + depth * (bumps as f64 * a / 2.0).sin().abs();
            Point::new(r * a.cos(), r * a.sin())
        })
        .collect();
    polyline_path(&pts, true)
}

/// A star with many short points.
fn starburst(points: u32, outer: f64, inner: f64) -> BezPath {
    crate::star_path(points, outer, inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bounds, flatten};

    #[test]
    fn gear_has_its_teeth_and_sizes() {
        let spec = GearSpec { teeth: 20, module: 2.0, pressure_angle: 20.0, bore: 8.0, keyway: 2.0 };
        let g = gear(&spec);
        let lines = flatten(&g, 0.01);
        assert_eq!(lines.len(), 2, "outline and bore");
        let b = bounds(&g).unwrap();
        assert!((b.width() - spec.outside_diameter()).abs() < 0.3, "{b:?}");
        // Count teeth: crossings of the pitch circle going outward.
        let outline = &lines[0].points;
        let r = spec.pitch_diameter() / 2.0;
        let outward = outline.iter().zip(outline.iter().cycle().skip(1)).filter(|(a, b)| a.to_vec2().hypot() < r && b.to_vec2().hypot() >= r).count();
        assert_eq!(outward, 20);
        // Tooth and gap are equally wide at the pitch circle.
        let crossings: Vec<f64> = outline.iter().zip(outline.iter().cycle().skip(1)).filter(|(a, b)| (a.to_vec2().hypot() - r) * (b.to_vec2().hypot() - r) <= 0.0).map(|(a, _)| a.to_vec2().atan2()).collect();
        let step = TAU / 20.0;
        let first = (crossings[1] - crossings[0]).rem_euclid(TAU);
        assert!((first - step / 2.0).abs() < 0.02, "tooth {first} vs half pitch {}", step / 2.0);
    }

    /// Writes a gear, the shape library and the sign shapes as polylines to `BC_MAKERS_DUMP`.
    #[test]
    #[ignore = "diagnostic"]
    fn dump() {
        let mut parts: Vec<BezPath> = vec![
            Affine::translate((35.0, 35.0)) * gear(&GearSpec { teeth: 18, module: 3.5, pressure_angle: 20.0, bore: 12.0, keyway: 3.0 }),
            Affine::translate((95.0, 35.0)) * gear(&GearSpec { teeth: 9, module: 3.5, pressure_angle: 20.0, bore: 6.0, keyway: 0.0 }),
        ];
        for (i, s) in LibraryShape::ALL.iter().enumerate() {
            parts.push(Affine::translate((150.0 + (i % 5) as f64 * 45.0, 20.0 + (i / 5) as f64 * 45.0)) * s.path(38.0));
        }
        for (i, s) in SignShape::ALL.iter().enumerate() {
            let mut p = sign_outline(*s, 80.0, 40.0, 8.0);
            p.extend(mounting_holes(2, 80.0, 40.0, 4.0, 4.0, *s == SignShape::Oval));
            parts.push(Affine::translate((45.0 + i as f64 * 90.0, 120.0)) * p);
        }
        let mut out = String::new();
        for p in parts {
            for l in flatten(&p, 0.05) {
                out += &l.points.iter().map(|q| format!("{:.2},{:.2}", q.x, q.y)).collect::<Vec<_>>().join(" ");
                out += if l.closed { " Z\n" } else { "\n" };
            }
        }
        std::fs::write(std::env::var("BC_MAKERS_DUMP").unwrap(), out).unwrap();
    }

    #[test]
    fn library_shapes_fit_their_size() {
        for s in LibraryShape::ALL {
            let b = bounds(&s.path(50.0)).unwrap();
            assert!((b.width().max(b.height()) - 50.0).abs() < 1e-6 && b.center().to_vec2().hypot() < 1e-6, "{s:?}: {b:?}");
        }
    }

    #[test]
    fn signs_and_holes() {
        for shape in SignShape::ALL {
            let b = bounds(&sign_outline(shape, 200.0, 100.0, 10.0)).unwrap();
            assert!((b.width() - 200.0).abs() < 0.1 && (b.height() - 100.0).abs() < 0.1, "{shape:?}: {b:?}");
        }
        let holes = flatten(&mounting_holes(4, 200.0, 100.0, 5.0, 6.0, false), 0.01);
        assert_eq!(holes.len(), 4);
        let b = bounds(&mounting_holes(4, 200.0, 100.0, 5.0, 6.0, false)).unwrap();
        assert!((b.x0 + 94.0).abs() < 1e-6 && (b.y1 - 44.0).abs() < 1e-6, "6 mm in from the edges: {b:?}");
    }
}

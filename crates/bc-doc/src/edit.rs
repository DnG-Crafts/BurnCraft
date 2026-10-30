//! Editing operations on a selection. Grouped shapes are treated as one unit.
//! Callers record an undo checkpoint before calling these.

use crate::{Document, ShapeKind};
use bc_geom::ops::BooleanOp;
use bc_geom::{Affine, BezPath, Point, Rect, Vec2};
use std::collections::{BTreeMap, BTreeSet};

pub type Selection = BTreeSet<u64>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
    /// Both centres at once: every unit centred on the selection's centre.
    Centers,
}

/// Selected shapes grouped into units: each group is one unit, ungrouped shapes are their own.
pub fn units(doc: &Document, sel: &Selection) -> Vec<Vec<u64>> {
    let mut groups: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut singles = Vec::new();
    for s in doc.shapes.iter().filter(|s| sel.contains(&s.id)) {
        match s.group {
            Some(g) => groups.entry(g).or_default().push(s.id),
            None => singles.push(vec![s.id]),
        }
    }
    singles.extend(groups.into_values());
    singles
}

/// Applies `t` (document space) to every unlocked selected shape.
pub fn transform(doc: &mut Document, sel: &Selection, t: Affine) {
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && !s.locked) {
        s.transform = t * s.transform;
    }
}

fn about(center: Point, t: Affine) -> Affine {
    Affine::translate(center.to_vec2()) * t * Affine::translate(-center.to_vec2())
}

pub fn translate(doc: &mut Document, sel: &Selection, delta: Vec2) {
    transform(doc, sel, Affine::translate(delta));
}

/// Rotates the selection about its centre, in degrees clockwise on screen.
pub fn rotate(doc: &mut Document, sel: &Selection, degrees: f64) {
    if let Some(b) = doc.bounds_of(sel.iter().copied()) {
        transform(doc, sel, about(b.center(), Affine::rotate(degrees.to_radians())));
    }
}

/// Mirrors the selection about its centre.
pub fn mirror(doc: &mut Document, sel: &Selection, horizontal: bool) {
    if let Some(b) = doc.bounds_of(sel.iter().copied()) {
        let s = if horizontal { Affine::scale_non_uniform(-1.0, 1.0) } else { Affine::scale_non_uniform(1.0, -1.0) };
        transform(doc, sel, about(b.center(), s));
    }
}

/// Resizes the selection's bounding box to `width` × `height`, keeping its top-left corner.
pub fn resize(doc: &mut Document, sel: &Selection, width: f64, height: f64) {
    let Some(b) = doc.bounds_of(sel.iter().copied()) else { return };
    let sx = if b.width() > 1e-9 { width.max(0.01) / b.width() } else { 1.0 };
    let sy = if b.height() > 1e-9 { height.max(0.01) / b.height() } else { 1.0 };
    transform(doc, sel, about(b.origin(), Affine::scale_non_uniform(sx, sy)));
}

/// Moves the selection so its bounding box's top-left is at `to`.
pub fn move_to(doc: &mut Document, sel: &Selection, to: Point) {
    if let Some(b) = doc.bounds_of(sel.iter().copied()) {
        translate(doc, sel, to - b.origin());
    }
}

/// Aligns each unit to the selection's overall bounding box.
pub fn align(doc: &mut Document, sel: &Selection, how: Align) {
    let Some(all) = doc.bounds_of(sel.iter().copied()) else { return };
    for unit in units(doc, sel) {
        let Some(b) = doc.bounds_of(unit.iter().copied()) else { continue };
        let delta = match how {
            Align::Left => Vec2::new(all.x0 - b.x0, 0.0),
            Align::Right => Vec2::new(all.x1 - b.x1, 0.0),
            Align::HCenter => Vec2::new(all.center().x - b.center().x, 0.0),
            Align::Top => Vec2::new(0.0, all.y0 - b.y0),
            Align::Bottom => Vec2::new(0.0, all.y1 - b.y1),
            Align::VCenter => Vec2::new(0.0, all.center().y - b.center().y),
            Align::Centers => all.center() - b.center(),
        };
        translate(doc, &unit.into_iter().collect(), delta);
    }
}

/// Spaces unit centres evenly between the outermost two (horizontally or vertically).
pub fn distribute(doc: &mut Document, sel: &Selection, horizontal: bool) {
    let mut items: Vec<(Vec<u64>, Rect)> = units(doc, sel).into_iter().filter_map(|u| doc.bounds_of(u.iter().copied()).map(|b| (u, b))).collect();
    if items.len() < 3 {
        return;
    }
    let key = |r: &Rect| if horizontal { r.center().x } else { r.center().y };
    items.sort_by(|a, b| key(&a.1).total_cmp(&key(&b.1)));
    let (first, last) = (key(&items[0].1), key(&items[items.len() - 1].1));
    let step = (last - first) / (items.len() - 1) as f64;
    for (i, (unit, b)) in items.iter().enumerate() {
        let d = first + step * i as f64 - key(b);
        let delta = if horizontal { Vec2::new(d, 0.0) } else { Vec2::new(0.0, d) };
        translate(doc, &unit.iter().copied().collect(), delta);
    }
}

/// Copies the selection with `transform` applied; copies keep their grouping (as new groups). Returns the copies' ids.
pub fn duplicate(doc: &mut Document, sel: &Selection, t: Affine) -> Selection {
    let originals: Vec<_> = doc.shapes.iter().filter(|s| sel.contains(&s.id)).cloned().collect();
    insert_copies(doc, &originals, t)
}

/// Adds another project's shapes to `doc` (File > Import of a BurnCraft project), returning the
/// new shapes' ids. Groups stay grouped. Layer settings come across only for layers this design
/// isn't using yet (or that the user added and may have set up), so they can't change what's
/// already here. Constraints come across pointing at the new shapes, and parameters are added
/// unless one of the same name exists.
pub fn merge_project(doc: &mut Document, other: &Document) -> Selection {
    let used = doc.used_layers();
    for i in other.used_layers() {
        if !used.contains(&i) && i < doc.layers.len() && i < other.layers.len() && !doc.layers[i].added {
            doc.layers[i] = other.layers[i].clone();
        }
    }
    doc.tool_layers_off();
    let mut ids: BTreeMap<u64, u64> = BTreeMap::new();
    let mut groups: BTreeMap<u64, u64> = BTreeMap::new();
    for s in &other.shapes {
        let id = doc.add_shape(s.kind.clone(), s.transform, s.layer);
        let new = doc.shape_mut(id).expect("just added");
        new.locked = s.locked;
        ids.insert(s.id, id);
        if let Some(g) = s.group {
            let ng = *groups.entry(g).or_insert_with(|| {
                let ng = doc.next_id;
                doc.next_id += 1;
                ng
            });
            doc.shape_mut(id).expect("just added").group = Some(ng);
        }
    }
    for p in &other.params.params {
        if !doc.params.params.iter().any(|q| q.name.trim() == p.name.trim()) {
            doc.params.params.push(p.clone());
        }
    }
    for c in &other.params.constraints {
        use crate::constraints::ConstraintKind;
        let kind = match &c.kind {
            ConstraintKind::Size { shape, axis, anchor } => ids.get(shape).map(|&shape| ConstraintKind::Size { shape, axis: *axis, anchor: *anchor }),
            ConstraintKind::Distance { axis, from, from_anchor, to, to_anchor } => {
                ids.get(from).zip(ids.get(to)).map(|(&from, &to)| ConstraintKind::Distance { axis: *axis, from, from_anchor: *from_anchor, to, to_anchor: *to_anchor })
            }
        };
        if let Some(kind) = kind {
            doc.params.constraints.push(crate::constraints::Constraint { kind, ..c.clone() });
        }
    }
    ids.into_values().collect()
}

/// Adds copies of `shapes` (e.g. from the clipboard) with `t` applied, giving them new ids and
/// new group ids (grouping among the copies is kept). Returns the copies' ids.
pub fn insert_copies(doc: &mut Document, shapes: &[crate::Shape], t: Affine) -> Selection {
    let mut new_groups: BTreeMap<u64, u64> = BTreeMap::new();
    let mut out = Selection::new();
    for s in shapes.iter().cloned() {
        let id = doc.add_shape(s.kind, t * s.transform, s.layer);
        if let Some(g) = s.group {
            let ng = *new_groups.entry(g).or_insert_with(|| {
                let ng = doc.next_id;
                doc.next_id += 1;
                ng
            });
            doc.shape_mut(id).expect("just added").group = Some(ng);
        }
        out.insert(id);
    }
    out
}

/// Grid of copies. `gap` is the spacing between copies' bounding boxes. Returns all copies (not the originals).
pub fn grid_array(doc: &mut Document, sel: &Selection, columns: usize, rows: usize, gap: Vec2) -> Selection {
    let Some(b) = doc.bounds_of(sel.iter().copied()) else { return Selection::new() };
    let mut all = Selection::new();
    for r in 0..rows.max(1) {
        for c in 0..columns.max(1) {
            if r == 0 && c == 0 {
                continue;
            }
            let d = Vec2::new(c as f64 * (b.width() + gap.x), r as f64 * (b.height() + gap.y));
            all.extend(duplicate(doc, sel, Affine::translate(d)));
        }
    }
    all
}

/// `count` copies (including the original) spaced evenly around `center`, optionally rotating each copy.
pub fn circular_array(doc: &mut Document, sel: &Selection, count: usize, center: Point, rotate_copies: bool) -> Selection {
    let Some(b) = doc.bounds_of(sel.iter().copied()) else { return Selection::new() };
    let mut all = Selection::new();
    for i in 1..count.max(1) {
        let angle = std::f64::consts::TAU * i as f64 / count as f64;
        let t = if rotate_copies {
            about(center, Affine::rotate(angle))
        } else {
            let moved = about(center, Affine::rotate(angle)) * b.center();
            Affine::translate(moved - b.center())
        };
        all.extend(duplicate(doc, sel, t));
    }
    all
}

fn closed_world_paths(doc: &Document, sel: &Selection) -> Vec<(u64, BezPath)> {
    doc.shapes
        .iter()
        .filter(|s| sel.contains(&s.id) && s.is_closed() && !matches!(s.kind, ShapeKind::Image { .. }))
        .map(|s| (s.id, s.world_path()))
        .collect()
}

/// Adds the outline offset by `distance` mm (negative = inward) around the union of the selected closed shapes.
/// Returns the new shape's id, or `None` if nothing remains.
pub fn offset(doc: &mut Document, sel: &Selection, distance: f64, layer: usize, delete_originals: bool) -> Option<u64> {
    let paths: Vec<BezPath> = closed_world_paths(doc, sel).into_iter().map(|(_, p)| p).collect();
    let base = match paths.len() {
        0 => return None,
        1 => paths[0].clone(),
        _ => bc_geom::ops::boolean(&paths, BooleanOp::Union)?,
    };
    let result = bc_geom::ops::offset(&base, distance)?;
    if delete_originals {
        doc.remove_where(|id| sel.contains(&id));
    }
    Some(doc.add_shape(ShapeKind::Path(result), Affine::IDENTITY, layer))
}

/// Combines the selected closed shapes. For subtraction the bottom-most shape (in draw order) is
/// the one cut from. The originals are replaced by the result, on the bottom shape's layer.
pub fn boolean(doc: &mut Document, sel: &Selection, op: BooleanOp) -> Option<u64> {
    let items = closed_world_paths(doc, sel);
    let layer = doc.shape(items.first()?.0)?.layer;
    let paths: Vec<BezPath> = items.iter().map(|(_, p)| p.clone()).collect();
    let result = bc_geom::ops::boolean(&paths, op)?;
    let used: Selection = items.iter().map(|(id, _)| *id).collect();
    doc.remove_where(|id| used.contains(&id));
    (!result.elements().is_empty()).then(|| doc.add_shape(ShapeKind::Path(result), Affine::IDENTITY, layer))
}

/// Packs the selected units into `area` in rows ("shelves"), tallest first, `spacing` mm apart.
/// With `allow_rotate`, a unit is turned 90° when that makes it lie flatter or fit the row.
/// Returns how many units were placed; units that don't fit are left where they were.
pub fn nest(doc: &mut Document, sel: &Selection, area: Rect, spacing: f64, allow_rotate: bool) -> usize {
    let mut items: Vec<(Selection, Rect)> =
        units(doc, sel).into_iter().map(|u| u.into_iter().collect::<Selection>()).filter_map(|u| doc.bounds_of(u.iter().copied()).map(|b| (u, b))).collect();
    if allow_rotate {
        // Lay each unit flat (wider than tall) so rows stay short.
        for (unit, b) in &mut items {
            if b.height() > b.width() * 1.05 {
                rotate(doc, unit, 90.0);
                *b = doc.bounds_of(unit.iter().copied()).expect("unit has bounds");
            }
        }
    }
    items.sort_by(|a, b| b.1.height().total_cmp(&a.1.height()).then(b.1.width().total_cmp(&a.1.width())));
    let (mut x, mut y, mut row_h) = (area.x0, area.y0, 0.0f64);
    let mut placed = 0;
    for (unit, mut b) in items {
        if x > area.x0 && x + b.width() > area.x1 {
            // Next row.
            x = area.x0;
            y += row_h + spacing;
            row_h = 0.0;
        }
        if allow_rotate && x + b.width() > area.x1 && b.height() <= area.x1 - x {
            rotate(doc, &unit, 90.0);
            b = doc.bounds_of(unit.iter().copied()).expect("unit has bounds");
        }
        if x + b.width() > area.x1 + 1e-9 || y + b.height() > area.y1 + 1e-9 {
            continue;
        }
        translate(doc, &unit, Point::new(x, y) - b.origin());
        x += b.width() + spacing;
        row_h = row_h.max(b.height());
        placed += 1;
    }
    placed
}

/// True-shape nesting of the selection into `area` (see [`bc_geom::nest`]). Groups move as one
/// part. `rotation_step` (degrees) sets which turns are tried (0 = none); `fine` uses a finer grid
/// (tighter, slower). Returns how many parts were placed; parts that don't fit stay where they were.
pub fn nest_true_shape(doc: &mut Document, sel: &Selection, area: Rect, spacing: f64, rotation_step: f64, fine: bool) -> usize {
    let parts: Vec<Selection> = units(doc, sel).into_iter().map(|u| u.into_iter().collect::<Selection>()).filter(|u| u.iter().all(|id| doc.shape(*id).is_some_and(|s| !s.locked))).collect();
    let outlines: Vec<Vec<bc_geom::BezPath>> = parts.iter().map(|u| doc.shapes.iter().filter(|s| u.contains(&s.id)).map(|s| s.world_path()).collect()).collect();
    let rotations: Vec<f64> = if rotation_step > 0.0 { (0..((360.0 / rotation_step).round() as usize).max(1)).map(|i| i as f64 * rotation_step).collect() } else { vec![0.0] };
    let cell = bc_geom::nest::NestOptions::auto_cell(area, if fine { 1_000_000.0 } else { 250_000.0 });
    let options = bc_geom::nest::NestOptions { area, spacing, rotations, cell };
    let placed = bc_geom::nest::nest(&outlines, &options);
    let mut count = 0;
    for (unit, t) in parts.iter().zip(placed) {
        if let Some(t) = t {
            transform(doc, unit, t);
            count += 1;
        }
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    ToFront,
    ToBack,
    Forward,
    Backward,
}

/// Changes draw order (and so cut order within a layer when optimisation is off).
pub fn reorder(doc: &mut Document, sel: &Selection, how: Order) {
    let n = doc.shapes.len();
    match how {
        Order::ToFront | Order::ToBack => {
            let (mut picked, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut doc.shapes).into_iter().partition(|s| sel.contains(&s.id));
            doc.shapes = if how == Order::ToFront { rest.into_iter().chain(picked).collect() } else { picked.extend(rest); picked };
        }
        Order::Forward => {
            for i in (0..n.saturating_sub(1)).rev() {
                if sel.contains(&doc.shapes[i].id) && !sel.contains(&doc.shapes[i + 1].id) {
                    doc.shapes.swap(i, i + 1);
                }
            }
        }
        Order::Backward => {
            for i in 1..n {
                if sel.contains(&doc.shapes[i].id) && !sel.contains(&doc.shapes[i - 1].id) {
                    doc.shapes.swap(i, i - 1);
                }
            }
        }
    }
}

/// Splits each selected shape into one shape per subpath. Returns the new selection.
pub fn break_apart(doc: &mut Document, sel: &Selection) -> Selection {
    let targets: Vec<_> = doc.shapes.iter().filter(|s| sel.contains(&s.id) && !matches!(s.kind, ShapeKind::Image { .. })).cloned().collect();
    let mut out: Selection = sel.iter().copied().filter(|id| !targets.iter().any(|t| t.id == *id)).collect();
    for shape in targets {
        let parts = split_subpaths(&shape.world_path());
        if parts.len() < 2 {
            out.insert(shape.id);
            continue;
        }
        doc.remove_where(|id| id == shape.id);
        for p in parts {
            out.insert(doc.add_shape(ShapeKind::Path(p), Affine::IDENTITY, shape.layer));
        }
    }
    out
}

fn split_subpaths(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if matches!(el, bc_geom::PathEl::MoveTo(_)) || out.is_empty() {
            out.push(BezPath::new());
        }
        out.last_mut().expect("pushed above").push(*el);
    }
    out.retain(|p| p.elements().len() > 1);
    out
}

/// Closes open subpaths whose ends are within `tolerance` mm (or all open subpaths if `tolerance` is infinite).
pub fn close_paths(doc: &mut Document, sel: &Selection, tolerance: f64) -> usize {
    let mut closed = 0;
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && matches!(s.kind, ShapeKind::Path(_))) {
        let world = s.world_path();
        let mut out = BezPath::new();
        for sub in split_subpaths(&world) {
            let els = sub.elements();
            let is_closed = els.last() == Some(&bc_geom::PathEl::ClosePath);
            let (first, last) = (els[0].end_point(), els.last().and_then(|e| e.end_point()));
            out.extend(sub.iter());
            if !is_closed && first.zip(last).is_some_and(|(a, b)| a.distance(b) <= tolerance) {
                out.close_path();
                closed += 1;
            }
        }
        s.kind = ShapeKind::Path(out);
        s.transform = Affine::IDENTITY;
    }
    closed
}

/// Reverses the cutting direction of the selected shapes (they become paths).
pub fn reverse(doc: &mut Document, sel: &Selection) {
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && !matches!(s.kind, ShapeKind::Image { .. })) {
        s.kind = ShapeKind::Path(s.world_path().reverse_subpaths());
        s.transform = Affine::IDENTITY;
    }
}

/// Joins open paths whose ends meet (within `tolerance` mm) into longer paths, closing any that
/// come back to their start. Returns the new selection.
pub fn auto_join(doc: &mut Document, sel: &Selection, tolerance: f64) -> Selection {
    let mut opens: Vec<(BezPath, usize)> = Vec::new();
    let mut keep = Selection::new();
    for s in doc.shapes.iter().filter(|s| sel.contains(&s.id)) {
        let is_open_path = matches!(s.kind, ShapeKind::Path(_)) && split_subpaths(&s.local_path()).len() == 1 && s.local_path().elements().last() != Some(&bc_geom::PathEl::ClosePath);
        if is_open_path {
            opens.push((s.world_path(), s.layer));
        } else {
            keep.insert(s.id);
        }
    }
    if opens.len() < 2 {
        return sel.clone();
    }
    let ends = |p: &BezPath| (p.elements()[0].end_point().unwrap(), p.elements().last().and_then(|e| e.end_point()).unwrap());
    // Greedily append any path whose start or end touches the current chain's end (or start).
    let mut chains: Vec<(BezPath, usize)> = Vec::new();
    while let Some((mut chain, layer)) = opens.pop() {
        loop {
            let (start, end) = ends(&chain);
            let hit = opens.iter().enumerate().find_map(|(i, (p, _))| {
                let (s, e) = ends(p);
                if s.distance(end) <= tolerance {
                    Some((i, false, false))
                } else if e.distance(end) <= tolerance {
                    Some((i, true, false))
                } else if e.distance(start) <= tolerance {
                    Some((i, false, true))
                } else if s.distance(start) <= tolerance {
                    Some((i, true, true))
                } else {
                    None
                }
            });
            let Some((i, flip, prepend)) = hit else { break };
            let (mut next, _) = opens.remove(i);
            if flip {
                next = next.reverse_subpaths();
            }
            let (a, b) = if prepend { (next, chain) } else { (chain, next) };
            let mut joined = a.clone();
            joined.extend(b.elements().iter().skip(1).copied());
            chain = joined;
        }
        let (start, end) = ends(&chain);
        if start.distance(end) <= tolerance {
            chain.close_path();
        }
        chains.push((chain, layer));
    }
    let originals: Selection = sel.iter().copied().filter(|id| !keep.contains(id)).collect();
    doc.remove_where(|id| originals.contains(&id));
    let mut out = keep;
    for (p, layer) in chains {
        out.insert(doc.add_shape(ShapeKind::Path(p), Affine::IDENTITY, layer));
    }
    out
}

/// Scales every unit to the width (or height) of the first one in draw order, keeping proportions.
pub fn make_same_size(doc: &mut Document, sel: &Selection, width: bool) {
    let us = units(doc, sel);
    let Some(target) = us.first().and_then(|u| doc.bounds_of(u.iter().copied())) else { return };
    let want = if width { target.width() } else { target.height() };
    for unit in us.into_iter().skip(1) {
        let unit: Selection = unit.into_iter().collect();
        let Some(b) = doc.bounds_of(unit.iter().copied()) else { continue };
        let have = if width { b.width() } else { b.height() };
        if have > 1e-9 {
            let s = want / have;
            resize(doc, &unit, b.width() * s, b.height() * s);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// Slides units toward `side` until they touch the selection's edge or the unit before them, `gap` mm apart.
pub fn dock(doc: &mut Document, sel: &Selection, side: Side, gap: f64) {
    let Some(all) = doc.bounds_of(sel.iter().copied()) else { return };
    let mut items: Vec<(Selection, Rect)> =
        units(doc, sel).into_iter().map(|u| u.into_iter().collect::<Selection>()).filter_map(|u| doc.bounds_of(u.iter().copied()).map(|b| (u, b))).collect();
    let key = |r: &Rect| match side {
        Side::Left => r.x0,
        Side::Right => -r.x1,
        Side::Up => r.y0,
        Side::Down => -r.y1,
    };
    items.sort_by(|a, b| key(&a.1).total_cmp(&key(&b.1)));
    let mut edge = match side {
        Side::Left => all.x0,
        Side::Right => all.x1,
        Side::Up => all.y0,
        Side::Down => all.y1,
    };
    for (unit, b) in items {
        let delta = match side {
            Side::Left => Vec2::new(edge - b.x0, 0.0),
            Side::Right => Vec2::new(edge - b.x1, 0.0),
            Side::Up => Vec2::new(0.0, edge - b.y0),
            Side::Down => Vec2::new(0.0, edge - b.y1),
        };
        translate(doc, &unit, delta);
        edge = match side {
            Side::Left => b.x0 + delta.x + b.width() + gap,
            Side::Right => b.x1 + delta.x - b.width() - gap,
            Side::Up => b.y0 + delta.y + b.height() + gap,
            Side::Down => b.y1 + delta.y - b.height() - gap,
        };
    }
}

/// Removes shapes that exactly duplicate an earlier shape (same layer and geometry within 0.01 mm).
pub fn delete_duplicates(doc: &mut Document) -> usize {
    let mut seen: Vec<(usize, Vec<Point>)> = Vec::new();
    let mut dupes = Selection::new();
    for s in &doc.shapes {
        let pts: Vec<Point> = bc_geom::flatten(&s.world_path(), 0.05).into_iter().flat_map(|l| l.points).collect();
        let same = |(layer, q): &(usize, Vec<Point>)| *layer == s.layer && q.len() == pts.len() && q.iter().zip(&pts).all(|(a, b)| a.distance(*b) < 0.01);
        if seen.iter().any(same) {
            dupes.insert(s.id);
        } else {
            seen.push((s.layer, pts));
        }
    }
    doc.remove_where(|id| dupes.contains(&id))
}

/// Rounds corners: rectangles get a corner radius; polygons, stars and straight-edged paths
/// become paths with filleted corners. Other shapes are left alone.
pub fn round_corners(doc: &mut Document, sel: &Selection, radius: f64) {
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && !s.locked) {
        match &s.kind {
            ShapeKind::Rect { width, height, .. } => s.kind = ShapeKind::Rect { width: *width, height: *height, corner_radius: radius },
            ShapeKind::Polygon { .. } | ShapeKind::Star { .. } | ShapeKind::Path(_) => {
                s.kind = ShapeKind::Path(bc_geom::fillet(&s.world_path(), radius));
                s.transform = Affine::IDENTITY;
            }
            _ => {}
        }
    }
}

/// A corner of a shape that the corner tool can round, re-round or make sharp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerHandle {
    pub shape: u64,
    pub corner: bc_geom::Corner,
}

/// The shape's outline as the corner tool sees it (document coordinates). Rectangles with a corner
/// radius come back with each corner rounded separately, so one corner can be changed on its own.
fn corner_path(s: &crate::Shape) -> Option<bc_geom::BezPath> {
    match &s.kind {
        ShapeKind::Rect { width, height, corner_radius } => Some(bc_geom::fillet(&(s.transform * bc_geom::rect_path(*width, *height, 0.0)), *corner_radius)),
        ShapeKind::Polygon { .. } | ShapeKind::Star { .. } | ShapeKind::Path(_) => Some(s.world_path()),
        _ => None,
    }
}

/// Corners of the visible, unlocked vector shapes (only those in `only`, if given).
pub fn corner_handles(doc: &Document, only: Option<&Selection>) -> Vec<CornerHandle> {
    doc.shapes
        .iter()
        .filter(|s| !s.locked && doc.layers[s.layer].visible && only.is_none_or(|sel| sel.contains(&s.id)))
        .filter_map(|s| corner_path(s).map(|p| (s.id, p)))
        .flat_map(|(shape, p)| bc_geom::corners(&p).into_iter().map(move |corner| CornerHandle { shape, corner }))
        .collect()
}

/// The corner whose handle is nearest `p`, within `tolerance` mm.
pub fn corner_near(doc: &Document, p: Point, tolerance: f64) -> Option<CornerHandle> {
    let nearby: Selection = doc.shapes.iter().filter(|s| s.bounds().is_some_and(|b| b.inflate(tolerance, tolerance).contains(p))).map(|s| s.id).collect();
    corner_handles(doc, Some(&nearby))
        .into_iter()
        .filter(|h| h.corner.handle.distance(p) <= tolerance)
        .min_by(|a, b| a.corner.handle.distance(p).total_cmp(&b.corner.handle.distance(p)))
}

/// Sets one corner (at `vertex`) of shape `id` to a rounding or chamfer of `radius`; 0 makes a rounded
/// corner sharp again. The shape becomes a path. Returns whether anything changed.
pub fn set_corner(doc: &mut Document, id: u64, vertex: Point, radius: f64, style: bc_geom::CornerStyle) -> bool {
    let Some(s) = doc.shapes.iter_mut().find(|s| s.id == id && !s.locked) else { return false };
    let Some(path) = corner_path(s) else { return false };
    let changed = bc_geom::set_corner(&path, vertex, radius.max(0.0), style);
    if changed == path {
        return false;
    }
    s.kind = ShapeKind::Path(changed);
    s.transform = Affine::IDENTITY;
    true
}
/// The merged region of the selected closed shapes and text, in document coordinates.
fn merged_region(doc: &Document, sel: &Selection) -> Option<BezPath> {
    let paths: Vec<BezPath> = doc
        .shapes
        .iter()
        .filter(|s| sel.contains(&s.id) && s.is_closed() && !matches!(s.kind, ShapeKind::Image { .. }))
        .map(|s| s.world_path())
        .collect();
    match paths.len() {
        0 => None,
        1 => Some(paths[0].clone()),
        _ => bc_geom::ops::boolean(&paths, bc_geom::ops::BooleanOp::Union),
    }
}

/// Inlay: from the selected closed shapes, a pocket (cut in the base, where they were) and a
/// matching insert (placed to the right). Both allow for the laser's `kerf` so the insert fits
/// tightly; `clearance` loosens the fit (mm, total). With `mirror`, the insert is flipped for
/// gluing in face down. The originals are replaced. Returns (pocket, insert) ids.
pub fn inlay(doc: &mut Document, sel: &Selection, kerf: f64, clearance: f64, mirror: bool) -> Option<(u64, u64)> {
    let region = merged_region(doc, sel)?;
    let layer = doc.shapes.iter().find(|s| sel.contains(&s.id))?.layer;
    let b = bc_geom::bounds(&region)?;
    let grow = |d: f64| if d.abs() < 1e-9 { Some(region.clone()) } else { bc_geom::ops::offset(&region, d) };
    // The cut removes half the kerf on each side of the line: draw the pocket smaller and the
    // insert larger by that much, then open both up by half the clearance.
    let pocket = grow(-kerf / 2.0 + clearance / 2.0)?;
    let mut insert = grow(kerf / 2.0 - clearance / 2.0)?;
    let c = b.center();
    if mirror {
        insert = Affine::translate(c.to_vec2()) * Affine::scale_non_uniform(-1.0, 1.0) * Affine::translate(-c.to_vec2()) * insert;
    }
    insert = Affine::translate((b.width() + 10.0, 0.0)) * insert;
    doc.remove_where(|id| sel.contains(&id));
    let p = doc.add_shape(ShapeKind::Path(pocket), Affine::IDENTITY, layer);
    let i = doc.add_shape(ShapeKind::Path(insert), Affine::IDENTITY, layer);
    Some((p, i))
}

/// A rubber stamp made from the selection.
pub struct Stamp {
    /// Engraved away at full depth.
    pub background: BezPath,
    /// A band around the design engraved shallower, so the raised parts have a sloped, sturdy base.
    pub shoulder: Option<BezPath>,
    /// The cut line around the stamp.
    pub outline: BezPath,
}

/// A rubber stamp from the selection. The design is mirrored (stamps print reversed) and inverted
/// (the background is burned, leaving the design raised), inside a border `margin` mm around it,
/// with an optional `shoulder` mm wide. `None` if nothing closed is selected.
pub fn rubber_stamp(doc: &Document, sel: &Selection, margin: f64, shoulder: f64) -> Option<Stamp> {
    use bc_geom::ops::{boolean, offset, BooleanOp};
    let region = merged_region(doc, sel)?;
    let b = bc_geom::bounds(&region)?;
    let c = b.center();
    let mirrored = Affine::translate(c.to_vec2()) * Affine::scale_non_uniform(-1.0, 1.0) * Affine::translate(-c.to_vec2()) * region;
    let outline = Affine::translate(c.to_vec2()) * bc_geom::rect_path(b.width() + 2.0 * margin, b.height() + 2.0 * margin, margin.min(3.0));
    let grown = if shoulder > 0.0 { offset(&mirrored, shoulder) } else { None };
    let (background, shoulder) = match grown {
        Some(grown) => {
            let band = boolean(&[grown.clone(), mirrored], BooleanOp::Subtract).and_then(|band| boolean(&[band, outline.clone()], BooleanOp::Intersect));
            (boolean(&[outline.clone(), grown], BooleanOp::Subtract)?, band)
        }
        None => (boolean(&[outline.clone(), mirrored], BooleanOp::Subtract)?, None),
    };
    Some(Stamp { background, shoulder, outline })
}

/// Scales `items` (as one unit, keeping proportions) to fit inside the bounds of `frame` less
/// `margin` mm all round, centred in it.
pub fn fit_into(doc: &mut Document, items: &Selection, frame: u64, margin: f64) -> bool {
    let (Some(f), Some(b)) = (doc.shape(frame).and_then(|s| s.bounds()), doc.bounds_of(items.iter().copied())) else { return false };
    if b.width() <= 0.0 && b.height() <= 0.0 {
        return false;
    }
    let room = |frame: f64, item: f64| if item > 1e-9 { (frame - 2.0 * margin).max(0.01) / item } else { f64::MAX };
    let s = room(f.width(), b.width()).min(room(f.height(), b.height()));
    let t = Affine::translate(f.center().to_vec2()) * Affine::scale(s) * Affine::translate(-b.center().to_vec2());
    transform(doc, items, t);
    true
}

/// For Fit Into Box: the frame (the largest selected vector shape) and the rest.
pub fn frame_and_items(doc: &Document, sel: &Selection) -> Option<(u64, Selection)> {
    guide_and_items(doc, sel)
}

/// Splits the selection into a grid of tiles with jointed seams (see [`bc_geom::tiles`]). Closed
/// shapes are cut along the seams; open lines and single-line text are clipped. Tiles are pulled
/// apart by `gap` mm and each is grouped. Images can't be split and are left as they are. Returns
/// each tile's shapes (empty tiles left out) and how many images were skipped.
pub fn split_into_tiles(doc: &mut Document, sel: &Selection, spec: &bc_geom::tiles::TileSpec, gap: f64) -> (Vec<Selection>, usize) {
    use bc_geom::ops::{boolean, clip_lines, BooleanOp};
    let Some(area) = doc.bounds_of(sel.iter().copied()) else { return (Vec::new(), 0) };
    let originals: Vec<crate::Shape> = doc.shapes.iter().filter(|s| sel.contains(&s.id) && !s.locked).cloned().collect();
    let images = originals.iter().filter(|s| matches!(s.kind, ShapeKind::Image { .. })).count();
    let parts: Vec<(&crate::Shape, BezPath, bool)> =
        originals.iter().filter(|s| !matches!(s.kind, ShapeKind::Image { .. })).map(|s| (s, s.world_path(), s.is_closed())).collect();
    let mut tiles = Vec::new();
    for (i, j, cell) in bc_geom::tiles::tile_cells(area, spec) {
        let shift = Affine::translate((i as f64 * gap, j as f64 * gap));
        let mut ids = Selection::new();
        for (s, path, closed) in &parts {
            let piece = if *closed { boolean(&[path.clone(), cell.clone()], BooleanOp::Intersect) } else { Some(clip_lines(path, &cell)) };
            if let Some(piece) = piece.filter(|p| !p.elements().is_empty()) {
                ids.insert(doc.add_shape(ShapeKind::Path(shift * piece), Affine::IDENTITY, s.layer));
            }
        }
        if !ids.is_empty() {
            if ids.len() > 1 {
                doc.group(&ids);
            }
            tiles.push(ids);
        }
    }
    let split: Selection = parts.iter().map(|(s, _, _)| s.id).collect();
    doc.remove_where(|id| split.contains(&id));
    (tiles, images)
}

/// Replaces a text shape's contents (new outline, size, font…), keeping its alignment point where
/// it was: with Align X = Left the left edge stays put, with Align Y = Bottom the bottom edge, and
/// so on. Curved text keeps its own placement. Returns false if `id` isn't text.
pub fn replace_text(doc: &mut Document, id: u64, kind: ShapeKind) -> bool {
    let Some(s) = doc.shape(id) else { return false };
    if !matches!(s.kind, ShapeKind::Text { .. }) {
        return false;
    }
    let old = s.bounds();
    let s = doc.shape_mut(id).expect("checked above");
    s.kind = kind;
    if let (Some(old), Some(new), ShapeKind::Text { style, .. }) = (old, s.bounds(), &s.kind)
        && style.curve.is_none()
    {
        // The new style decides the anchor for both, so changing the alignment itself doesn't move the text.
        let shift = style.anchor(old) - style.anchor(new);
        s.transform = Affine::translate(shift) * s.transform;
    }
    true
}

/// Adds stencil bridges to every selected closed shape or text that has holes (see
/// [`bc_geom::ops::stencil_bridges`]); those become paths. Returns how many shapes changed.
pub fn stencil(doc: &mut Document, sel: &Selection, width: f64, horizontal: bool) -> usize {
    let mut changed = 0;
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && !s.locked && s.is_closed()) {
        if !matches!(s.kind, ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. } | ShapeKind::Polygon { .. } | ShapeKind::Star { .. } | ShapeKind::Path(_) | ShapeKind::Text { .. }) {
            continue;
        }
        if let Some(p) = bc_geom::ops::stencil_bridges(&s.world_path(), width, horizontal) {
            s.kind = ShapeKind::Path(p);
            s.transform = Affine::IDENTITY;
            changed += 1;
        }
    }
    changed
}

/// For Place on Each Slot: the selected closed shapes on tool layers (the jig's slots) and the rest
/// (the design). `None` unless both are there.
pub fn slots_and_design(doc: &Document, sel: &Selection) -> Option<(Vec<u64>, Selection)> {
    let slots: Vec<u64> = doc.shapes.iter().filter(|s| sel.contains(&s.id) && crate::is_tool_layer(s.layer) && s.is_closed()).map(|s| s.id).collect();
    let design: Selection = sel.iter().copied().filter(|id| !slots.contains(id)).collect();
    (!slots.is_empty() && !design.is_empty()).then_some((slots, design))
}

/// How a slot sits: its smallest rotated bounding rectangle, long side first.
pub fn slot_rect(doc: &Document, slot: u64) -> Option<bc_geom::fit::OrientedRect> {
    let path = doc.shape(slot)?.world_path();
    let points: Vec<Point> = bc_geom::flatten(&path, 0.05).into_iter().flat_map(|l| l.points).collect();
    bc_geom::fit::min_area_rect(&points)
}

/// Batch engraving on a jig: copies `design` (as one unit) onto the centre of every slot, turned so
/// its long side runs along the slot's. With `fit`, each copy is scaled to fit inside the slot less
/// `margin` mm all round. The original design is removed unless `keep_original`. Copies of several
/// shapes are grouped. Returns the new shapes' ids.
pub fn place_on_slots(doc: &mut Document, slots: &[u64], design: &Selection, fit: bool, margin: f64, keep_original: bool) -> Vec<u64> {
    use std::f64::consts::{FRAC_PI_2, PI};
    let Some(b) = doc.bounds_of(design.iter().copied()) else { return Vec::new() };
    let (dw, dh) = (b.width(), b.height());
    let originals: Vec<crate::Shape> = doc.shapes.iter().filter(|s| design.contains(&s.id)).cloned().collect();
    let rects: Vec<_> = slots.iter().filter_map(|id| slot_rect(doc, *id)).collect();
    let mut new_ids = Vec::new();
    for r in rects {
        let mut angle = if dw >= dh { r.angle } else { r.angle + FRAC_PI_2 };
        // Keep the copy the right way up: of the two turns that line it up, use the smaller.
        angle = (angle + FRAC_PI_2).rem_euclid(PI) - FRAC_PI_2;
        let scale = if fit {
            let (long, short) = (dw.max(dh), dw.min(dh));
            let room = |slot: f64, item: f64| if item > 1e-9 { (slot - 2.0 * margin).max(0.0) / item } else { f64::MAX };
            let s = room(r.width, long).min(room(r.height, short));
            if s.is_finite() && s > 0.0 && s < f64::MAX { s } else { 1.0 }
        } else {
            1.0
        };
        let place = Affine::translate(r.center.to_vec2()) * Affine::rotate(angle) * Affine::scale(scale) * Affine::translate(-b.center().to_vec2());
        let ids = insert_copies(doc, &originals, place);
        if ids.len() > 1 {
            doc.group(&ids);
        }
        new_ids.extend(ids);
    }
    if !keep_original && !new_ids.is_empty() {
        doc.remove_where(|id| design.contains(&id));
    }
    new_ids
}

/// For Copy Along Path: the guide (the largest selected vector shape) and the items to copy (the rest).
pub fn guide_and_items(doc: &Document, sel: &Selection) -> Option<(u64, Selection)> {
    if sel.len() < 2 {
        return None;
    }
    let size = |s: &crate::Shape| s.bounds().map(|b| b.width().hypot(b.height())).unwrap_or(0.0);
    let guide = doc.shapes.iter().filter(|s| sel.contains(&s.id) && cuttable(s)).max_by(|a, b| size(a).total_cmp(&size(b)))?;
    let items: Selection = sel.iter().copied().filter(|id| *id != guide.id).collect();
    Some((guide.id, items))
}

/// Places `count` copies of `items` (as one unit, centred) evenly along the outline of `guide`:
/// all the way round a closed outline, end to end along an open one. With `rotate`, each copy turns
/// to follow the path. Copies of several items are grouped. Returns the new shapes' ids.
pub fn copy_along_path(doc: &mut Document, items: &Selection, guide: u64, count: usize, rotate: bool) -> Vec<u64> {
    let Some(path) = doc.shape(guide).map(|s| s.world_path()) else { return Vec::new() };
    let Some(line) = bc_geom::flatten(&path, 0.02).into_iter().next() else { return Vec::new() };
    let Some(center) = doc.bounds_of(items.iter().copied()).map(|b| b.center()) else { return Vec::new() };
    let mut pts = line.points.clone();
    if line.closed && pts.first() != pts.last() {
        pts.push(pts[0]);
    }
    let mut lengths = vec![0.0];
    for w in pts.windows(2) {
        lengths.push(lengths.last().unwrap() + w[0].distance(w[1]));
    }
    let total = *lengths.last().unwrap();
    if count == 0 || total <= 0.0 || pts.len() < 2 {
        return Vec::new();
    }
    let at = |s: f64| -> (Point, f64) {
        let i = lengths.partition_point(|&l| l <= s).clamp(1, pts.len() - 1);
        let seg = (lengths[i] - lengths[i - 1]).max(1e-12);
        let p = pts[i - 1].lerp(pts[i], ((s - lengths[i - 1]) / seg).clamp(0.0, 1.0));
        (p, (pts[i] - pts[i - 1]).atan2())
    };
    let originals: Vec<crate::Shape> = doc.shapes.iter().filter(|s| items.contains(&s.id)).cloned().collect();
    let mut new_ids = Vec::new();
    for k in 0..count {
        let s = match (line.closed, count) {
            (true, _) => total * k as f64 / count as f64,
            (false, 1) => total / 2.0,
            (false, _) => total * k as f64 / (count - 1) as f64,
        };
        let (p, angle) = at(s);
        let place = Affine::translate(p.to_vec2()) * Affine::rotate(if rotate { angle } else { 0.0 }) * Affine::translate(-center.to_vec2());
        let ids: Selection = originals.iter().map(|o| doc.add_shape(o.kind.clone(), place * o.transform, o.layer)).collect();
        if ids.len() > 1 {
            doc.group(&ids);
        }
        new_ids.extend(ids);
    }
    new_ids
}

/// With one image and one closed vector shape selected: clips the image to the shape (a rectangle
/// crops it) and removes the shape. Returns the image's id.
pub fn clip_image(doc: &mut Document, sel: &Selection) -> Option<u64> {
    if sel.len() != 2 {
        return None;
    }
    let image = doc.shapes.iter().find(|s| sel.contains(&s.id) && matches!(s.kind, ShapeKind::Image { .. }))?;
    let mask = doc.shapes.iter().find(|s| sel.contains(&s.id) && s.id != image.id && cuttable(s) && s.is_closed())?;
    let (image_id, mask_id) = (image.id, mask.id);
    let local = image.transform.inverse() * mask.world_path();
    if let Some(ShapeKind::Image { clip, .. }) = doc.shape_mut(image_id).map(|s| &mut s.kind) {
        *clip = Some(local);
    }
    doc.remove_where(|id| id == mask_id);
    Some(image_id)
}

/// Removes the clip from the selected images. Returns whether any had one.
pub fn unclip_images(doc: &mut Document, sel: &Selection) -> bool {
    let mut any = false;
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id)) {
        if let ShapeKind::Image { clip, .. } = &mut s.kind {
            any |= clip.take().is_some();
        }
    }
    any
}

/// Vector shapes the trim and knife tools work on.
fn cuttable(s: &crate::Shape) -> bool {
    matches!(s.kind, ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. } | ShapeKind::Polygon { .. } | ShapeKind::Star { .. } | ShapeKind::Path(_))
}

/// The visible, unlocked vector shape whose outline passes within `tolerance` of `p`.
pub fn outline_near(doc: &Document, p: Point, tolerance: f64) -> Option<u64> {
    doc.shapes
        .iter()
        .filter(|s| !s.locked && doc.layers[s.layer].visible && cuttable(s))
        .map(|s| (s.id, bc_geom::distance_to_outline(&s.world_path(), p)))
        .filter(|(_, d)| *d <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Trim tool: deletes the piece of the outline under `p` between its crossings with other shapes
/// (or itself). A shape trimmed to nothing is removed. Returns the shape changed.
pub fn trim_at(doc: &mut Document, p: Point, tolerance: f64) -> Option<u64> {
    let id = outline_near(doc, p, tolerance)?;
    let target = doc.shape(id)?.world_path();
    let cutters: Vec<bc_geom::BezPath> = doc.shapes.iter().filter(|s| s.id != id && doc.layers[s.layer].visible && cuttable(s)).map(|s| s.world_path()).collect();
    let trimmed = bc_geom::cut::trim(&target, &cutters, p, tolerance)?;
    if trimmed.elements().is_empty() {
        doc.remove_where(|x| x == id);
    } else if let Some(s) = doc.shape_mut(id) {
        s.kind = ShapeKind::Path(trimmed);
        s.transform = Affine::IDENTITY;
    }
    Some(id)
}

/// Knife tool: splits every visible, unlocked vector shape the line `a`→`b` cuts through. Pieces stay
/// on the same layer and group. Returns the ids of all pieces.
pub fn knife(doc: &mut Document, a: Point, b: Point) -> Vec<u64> {
    let targets: Vec<(u64, Vec<bc_geom::BezPath>)> = doc
        .shapes
        .iter()
        .filter(|s| !s.locked && doc.layers[s.layer].visible && cuttable(s))
        .filter_map(|s| bc_geom::cut::knife(&s.world_path(), a, b).map(|pieces| (s.id, pieces)))
        .collect();
    let mut ids = Vec::new();
    for (id, pieces) in targets {
        let Some((layer, group)) = doc.shape(id).map(|s| (s.layer, s.group)) else { continue };
        let mut pieces = pieces.into_iter();
        if let (Some(first), Some(s)) = (pieces.next(), doc.shape_mut(id)) {
            s.kind = ShapeKind::Path(first);
            s.transform = Affine::IDENTITY;
            ids.push(id);
        }
        for piece in pieces {
            let new = doc.add_shape(ShapeKind::Path(piece), Affine::IDENTITY, layer);
            if let Some(s) = doc.shape_mut(new) {
                s.group = group;
            }
            ids.push(new);
        }
    }
    ids
}

/// Shapes with at least one open subpath.
pub fn open_shapes(doc: &Document) -> Selection {
    doc.shapes.iter().filter(|s| !s.is_closed()).map(|s| s.id).collect()
}

/// Replaces parametric shapes and text with plain paths (images are left alone).
pub fn convert_to_path(doc: &mut Document, sel: &Selection) {
    for s in doc.shapes.iter_mut().filter(|s| sel.contains(&s.id) && !matches!(s.kind, ShapeKind::Image { .. }) && (!matches!(s.kind, ShapeKind::Path(_)) || !s.effects.is_empty())) {
        s.kind = ShapeKind::Path(s.world_path());
        s.transform = Affine::IDENTITY;
        s.effects.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(doc: &mut Document, x: f64, y: f64, w: f64, h: f64) -> u64 {
        doc.add_shape(ShapeKind::Rect { width: w, height: h, corner_radius: 0.0 }, Affine::translate((x + w / 2.0, y + h / 2.0)), 0)
    }

    fn b(doc: &Document, id: u64) -> Rect {
        doc.shape(id).unwrap().bounds().unwrap()
    }

    #[test]
    fn splitting_into_jointed_tiles() {
        use bc_geom::tiles::{Joint, TileSpec};
        let mut doc = Document::default();
        let panel = rect(&mut doc, 0.0, 0.0, 400.0, 100.0);
        let mut line = BezPath::new();
        line.move_to((10.0, 50.0));
        line.line_to((390.0, 50.0));
        let stroke = doc.add_shape(ShapeKind::Path(line), Affine::IDENTITY, 1);
        let spec = TileSpec { cols: 2, rows: 1, joint: Joint::Dovetail, size: 10.0, spacing: 40.0 };
        let (tiles, images) = split_into_tiles(&mut doc, &[panel, stroke].into(), &spec, 20.0);
        assert_eq!((tiles.len(), images), (2, 0));
        assert!(doc.shape(panel).is_none() && doc.shape(stroke).is_none());
        // Each tile has its half of the panel and of the line, on the original layers.
        for t in &tiles {
            assert_eq!(t.len(), 2);
            let layers: BTreeSet<usize> = t.iter().map(|id| doc.shape(*id).unwrap().layer).collect();
            assert_eq!(layers, [0, 1].into());
        }
        let area: f64 = tiles.iter().flatten().filter(|id| doc.shape(**id).unwrap().layer == 0).map(|id| bc_geom::ops::area(&doc.shape(*id).unwrap().world_path())).sum();
        assert!((area - 40_000.0).abs() < 1.0, "{area}");
        // The second tile is pulled 20 mm to the right.
        let right = doc.bounds_of(tiles[1].iter().copied()).unwrap();
        assert!(right.x1 > 419.0, "{right:?}");
    }

    #[test]
    fn text_grows_from_its_alignment_point() {
        use crate::{TextAlign, TextStyle, TextVAlign};
        let text = |w: f64, h: f64, align, valign| {
            let style = TextStyle { align, valign, ..Default::default() };
            ShapeKind::Text { text: "x".into(), font: "f".into(), height: h, bold: false, italic: false, style, outline: bc_geom::rect_path(w, h, 0.0) }
        };
        let mut doc = Document::default();
        let id = doc.add_shape(text(40.0, 10.0, TextAlign::Left, TextVAlign::Top), Affine::translate((100.0, 50.0)), 0);
        let before = b(&doc, id);
        // Twice as wide and tall: the top-left corner stays.
        assert!(replace_text(&mut doc, id, text(80.0, 20.0, TextAlign::Left, TextVAlign::Top)));
        let after = b(&doc, id);
        assert!((after.x0 - before.x0).abs() < 1e-9 && (after.y0 - before.y0).abs() < 1e-9 && (after.width() - 80.0).abs() < 1e-9, "{after:?}");
        // Right / bottom: that corner stays instead.
        let id = doc.add_shape(text(40.0, 10.0, TextAlign::Right, TextVAlign::Bottom), Affine::translate((100.0, 50.0)), 0);
        let before = b(&doc, id);
        replace_text(&mut doc, id, text(80.0, 20.0, TextAlign::Right, TextVAlign::Bottom));
        let after = b(&doc, id);
        assert!((after.x1 - before.x1).abs() < 1e-9 && (after.y1 - before.y1).abs() < 1e-9, "{after:?}");
        // Only changing the alignment doesn't move the text.
        let id = doc.add_shape(text(40.0, 10.0, TextAlign::Center, TextVAlign::Middle), Affine::translate((100.0, 50.0)), 0);
        let before = b(&doc, id);
        replace_text(&mut doc, id, text(40.0, 10.0, TextAlign::Left, TextVAlign::Bottom));
        assert_eq!(b(&doc, id), before);
        // Not text: nothing happens.
        let r = rect(&mut doc, 0.0, 0.0, 5.0, 5.0);
        assert!(!replace_text(&mut doc, r, text(1.0, 1.0, TextAlign::Left, TextVAlign::Top)));
    }

    #[test]
    fn inlay_pocket_and_insert_allow_for_kerf() {
        let mut doc = Document::default();
        let id = rect(&mut doc, 0.0, 0.0, 20.0, 10.0);
        let (p, i) = inlay(&mut doc, &[id].into(), 0.2, 0.0, false).unwrap();
        assert!(doc.shape(id).is_none());
        let (pb, ib) = (b(&doc, p), b(&doc, i));
        assert!((pb.width() - 19.8).abs() < 1e-3 && (ib.width() - 20.2).abs() < 1e-3, "{pb:?} {ib:?}");
        assert!(ib.x0 > pb.x1, "the insert goes beside the pocket");
        // Clearance opens the pocket and shrinks the insert.
        let id = rect(&mut doc, 0.0, 50.0, 20.0, 10.0);
        let (p, i) = inlay(&mut doc, &[id].into(), 0.2, 0.1, false).unwrap();
        assert!((b(&doc, p).width() - 19.9).abs() < 1e-3 && (b(&doc, i).width() - 20.1).abs() < 1e-3);
    }

    #[test]
    fn stamp_burns_the_background_of_a_mirrored_design() {
        let mut doc = Document::default();
        // An L shape, off-centre to the left of its bounds when mirrored.
        let a = rect(&mut doc, 0.0, 0.0, 4.0, 20.0);
        let c = rect(&mut doc, 0.0, 16.0, 12.0, 4.0);
        let stamp = rubber_stamp(&doc, &[a, c].into(), 3.0, 0.0).unwrap();
        assert!(stamp.shoulder.is_none());
        let (bg, border) = (stamp.background, stamp.outline);
        let bb = bc_geom::bounds(&border).unwrap();
        assert!((bb.width() - 18.0).abs() < 1e-6 && (bb.height() - 26.0).abs() < 1e-6);
        // The upright of the L is now on the right: that spot isn't burned, the left side is.
        assert!(!bc_geom::contains(&bg, Point::new(10.0, 5.0)) && bc_geom::contains(&bg, Point::new(2.0, 5.0)));
        let area = bc_geom::ops::area(&bg);
        // Border less the L (80 + 48 - 16 overlap) less the border's four 3 mm rounded corners.
        let corners = 4.0 * (9.0 - std::f64::consts::PI * 9.0 / 4.0);
        assert!((area - (18.0 * 26.0 - 112.0 - corners)).abs() < 0.2, "{area}");
        // A shoulder takes a band out of the background and engraves it separately.
        let stamp = rubber_stamp(&doc, &[a, c].into(), 3.0, 1.0).unwrap();
        let (bg2, band) = (bc_geom::ops::area(&stamp.background), bc_geom::ops::area(&stamp.shoulder.unwrap()));
        assert!(bg2 < area - 10.0 && (bg2 + band - area).abs() < 0.2, "{bg2} + {band} vs {area}");
    }

    #[test]
    fn fit_into_a_box() {
        let mut doc = Document::default();
        let frame = rect(&mut doc, 0.0, 0.0, 100.0, 50.0);
        let item = rect(&mut doc, 200.0, 200.0, 10.0, 10.0);
        let (f, items) = frame_and_items(&doc, &[frame, item].into()).unwrap();
        assert_eq!(f, frame);
        assert!(fit_into(&mut doc, &items, f, 5.0));
        let r = b(&doc, item);
        assert!((r.width() - 40.0).abs() < 1e-6 && (r.center() - Point::new(50.0, 25.0)).hypot() < 1e-6, "{r:?}");
    }

    #[test]
    fn designs_go_on_every_jig_slot() {
        let mut doc = Document::default();
        let t1 = crate::CUT_LAYER_COUNT;
        // Two slots on Tool1: one level 40×20, one standing upright (turned 90°) 30×15.
        let slot_a = doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 0.0 }, Affine::translate((50.0, 50.0)), t1);
        let slot_b = doc.add_shape(ShapeKind::Rect { width: 30.0, height: 15.0, corner_radius: 0.0 }, Affine::translate((150.0, 50.0)) * Affine::rotate(std::f64::consts::FRAC_PI_2), t1);
        let design = rect(&mut doc, 0.0, 0.0, 20.0, 10.0);
        let sel: Selection = [slot_a, slot_b, design].into();
        let (slots, items) = slots_and_design(&doc, &sel).unwrap();
        assert_eq!((slots.len(), items.len()), (2, 1));
        let ids = place_on_slots(&mut doc, &slots, &items, false, 0.0, false);
        assert_eq!(ids.len(), 2);
        assert!(doc.shape(design).is_none(), "the original goes");
        let (p, q) = (b(&doc, ids[0]), b(&doc, ids[1]));
        assert!((p.center() - Point::new(50.0, 50.0)).hypot() < 1e-6 && (p.width() - 20.0).abs() < 1e-6);
        // In the upright slot the copy stands up too.
        assert!((q.center() - Point::new(150.0, 50.0)).hypot() < 1e-6 && (q.width() - 10.0).abs() < 1e-6 && (q.height() - 20.0).abs() < 1e-6, "{q:?}");
        // Fitting scales each copy to the slot less the margin: 30×15 slot, 1 mm margin → 28/20 = 1.4, 13/10 = 1.3.
        let design = rect(&mut doc, 0.0, 0.0, 20.0, 10.0);
        let ids = place_on_slots(&mut doc, &[slot_b], &[design].into(), true, 1.0, true);
        let q = b(&doc, ids[0]);
        assert!((q.height() - 26.0).abs() < 1e-6 && (q.width() - 13.0).abs() < 1e-6, "{q:?}");
        assert!(doc.shape(design).is_some(), "kept when asked");
    }

    #[test]
    fn rounding_single_corners_with_different_radii() {
        let mut doc = Document::default();
        let id = rect(&mut doc, 10.0, 10.0, 40.0, 20.0);
        assert_eq!(corner_handles(&doc, None).len(), 4);
        // Click 1 mm away from the top-left corner.
        let h = corner_near(&doc, Point::new(11.0, 10.5), 2.0).unwrap();
        assert_eq!((h.shape, h.corner.vertex, h.corner.radius), (id, Point::new(10.0, 10.0), None));
        assert!(set_corner(&mut doc, id, h.corner.vertex, 5.0, bc_geom::CornerStyle::Round));
        let other = corner_near(&doc, Point::new(49.5, 29.5), 2.0).unwrap();
        assert_eq!(other.corner.vertex, Point::new(50.0, 30.0));
        assert!(set_corner(&mut doc, id, other.corner.vertex, 2.0, bc_geom::CornerStyle::Round));
        let path = doc.shape(id).unwrap().world_path();
        assert!(!bc_geom::contains(&path, Point::new(11.0, 11.0)), "big radius at top-left");
        assert!(bc_geom::contains(&path, Point::new(49.0, 29.0)), "small radius at bottom-right");
        assert!(bc_geom::contains(&path, Point::new(49.8, 10.2)), "top-right untouched");
        assert!(near(b(&doc, id).width(), 40.0));

        // The rounded corner is offered again (its handle sits on the curve) and can be changed…
        let rounded = corner_handles(&doc, None).into_iter().find(|h| h.corner.vertex == Point::new(10.0, 10.0)).unwrap();
        assert!(near(rounded.corner.radius.unwrap(), 5.0));
        let again = corner_near(&doc, rounded.corner.handle, 2.0).unwrap();
        assert_eq!(again.corner.vertex, Point::new(10.0, 10.0));
        assert!(set_corner(&mut doc, id, again.corner.vertex, 1.0, bc_geom::CornerStyle::Round));
        let path = doc.shape(id).unwrap().world_path();
        assert!(bc_geom::contains(&path, Point::new(10.5, 10.5)), "now a small radius");
        // …or made sharp with 0.
        assert!(set_corner(&mut doc, id, again.corner.vertex, 0.0, bc_geom::CornerStyle::Round));
        assert!(corner_handles(&doc, None).iter().find(|h| h.corner.vertex == Point::new(10.0, 10.0)).unwrap().corner.radius.is_none());
        assert!(!set_corner(&mut doc, id, again.corner.vertex, 0.0, bc_geom::CornerStyle::Round), "already sharp: no change");

        // Locked shapes have no corners to change.
        doc.shapes[0].locked = true;
        assert!(corner_near(&doc, Point::new(50.0, 10.0), 2.0).is_none());
    }

    #[test]
    fn trim_and_knife_on_the_document() {
        let mut doc = Document::default();
        // A long bar crossed by two short bars: trim the middle of the long one's top edge.
        let long = rect(&mut doc, 0.0, 0.0, 100.0, 10.0);
        rect(&mut doc, 30.0, -5.0, 5.0, 20.0);
        rect(&mut doc, 60.0, -5.0, 5.0, 20.0);
        assert_eq!(trim_at(&mut doc, Point::new(47.0, 0.2), 1.0), Some(long));
        let outline = doc.shape(long).unwrap().world_path();
        assert!(bc_geom::distance_to_outline(&outline, Point::new(47.0, 0.0)) > 1.0, "the piece is gone");
        assert!(bc_geom::distance_to_outline(&outline, Point::new(10.0, 0.0)) < 1e-6, "the rest stays");
        assert!(trim_at(&mut doc, Point::new(47.0, 40.0), 1.0).is_none(), "nothing there");

        // Knife a square in two; both halves stay on its layer.
        let mut doc = Document::default();
        let sq = rect(&mut doc, 0.0, 0.0, 40.0, 40.0);
        doc.shapes[0].layer = 3;
        let pieces = knife(&mut doc, Point::new(20.0, -5.0), Point::new(20.0, 45.0));
        assert_eq!(pieces.len(), 2);
        assert!(pieces.contains(&sq));
        assert!(doc.shapes.iter().all(|s| s.layer == 3));
        let widths: Vec<f64> = pieces.iter().map(|id| b(&doc, *id).width()).collect();
        assert!(widths.iter().all(|w| near(*w, 20.0)), "{widths:?}");
    }

    #[test]
    fn true_shape_nesting_keeps_groups_together() {
        let mut doc = Document::default();
        let big = rect(&mut doc, 300.0, 300.0, 30.0, 20.0);
        let small = rect(&mut doc, 340.0, 300.0, 10.0, 10.0);
        doc.group(&[big, small].into_iter().collect());
        let circle = doc.add_shape(ShapeKind::Ellipse { radius_x: 10.0, radius_y: 10.0 }, Affine::translate((500.0, 350.0)), 0);
        let sel: Selection = [big, small, circle].into_iter().collect();
        let apart = |doc: &Document| b(doc, big).center().distance(b(doc, small).center());
        let before = apart(&doc);
        assert_eq!(nest_true_shape(&mut doc, &sel, Rect::new(0.0, 0.0, 200.0, 100.0), 2.0, 90.0, false), 2);
        for id in [big, small, circle] {
            let r = b(&doc, id);
            assert!(r.x0 >= -1e-6 && r.y0 >= -1e-6 && r.x1 <= 200.0 + 1e-6 && r.y1 <= 100.0 + 1e-6, "inside the area: {r:?}");
        }
        assert!(near(apart(&doc), before), "the group moved as one piece");
    }
    #[test]
    fn copies_go_round_a_circle() {
        let mut doc = Document::default();
        let circle = doc.add_shape(ShapeKind::Ellipse { radius_x: 50.0, radius_y: 50.0 }, Affine::translate((100.0, 100.0)), 0);
        let star = doc.add_shape(ShapeKind::Star { points: 5, outer: 3.0, inner: 1.5 }, Affine::translate((10.0, 10.0)), 1);
        let (guide, items) = guide_and_items(&doc, &[circle, star].into_iter().collect()).unwrap();
        assert_eq!((guide, items.len()), (circle, 1));
        let copies = copy_along_path(&mut doc, &items, guide, 8, true);
        assert_eq!(copies.len(), 8);
        for id in &copies {
            let c = b(&doc, *id).center();
            assert!((c.distance(Point::new(100.0, 100.0)) - 50.0).abs() < 0.6, "on the circle: {c:?}");
            assert_eq!(doc.shape(*id).unwrap().layer, 1, "keeps its layer");
        }
        // Open line: first and last copies at the ends.
        let mut doc = Document::default();
        let mut l = bc_geom::BezPath::new();
        l.move_to((0.0, 0.0));
        l.line_to((90.0, 0.0));
        let line = doc.add_shape(ShapeKind::Path(l), Affine::IDENTITY, 0);
        let dot = doc.add_shape(ShapeKind::Ellipse { radius_x: 1.0, radius_y: 1.0 }, Affine::translate((5.0, 50.0)), 0);
        let copies = copy_along_path(&mut doc, &[dot].into_iter().collect(), line, 4, false);
        let xs: Vec<f64> = copies.iter().map(|id| b(&doc, *id).center().x).collect();
        assert!(xs.iter().zip([0.0, 30.0, 60.0, 90.0]).all(|(a, b)| (a - b).abs() < 1e-6), "{xs:?}");
    }

    #[test]
    fn one_corner_of_a_rounded_rectangle() {
        let mut doc = Document::default();
        let id = doc.add_shape(ShapeKind::Rect { width: 40.0, height: 20.0, corner_radius: 4.0 }, Affine::translate((30.0, 20.0)), 0);
        let handles = corner_handles(&doc, None);
        assert_eq!(handles.len(), 4);
        assert!(handles.iter().all(|h| h.corner.radius.is_some_and(|r| near(r, 4.0))));
        // Make just the top-left sharp; the others keep their 4 mm.
        assert!(set_corner(&mut doc, id, Point::new(10.0, 10.0), 0.0, bc_geom::CornerStyle::Round));
        let radii: Vec<Option<f64>> = corner_handles(&doc, None).iter().map(|h| h.corner.radius).collect();
        assert_eq!(radii.iter().filter(|r| r.is_none()).count(), 1);
        assert_eq!(radii.iter().filter(|r| r.is_some_and(|r| near(r, 4.0))).count(), 3);
    }
    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn importing_a_project_adds_its_shapes() {
        use crate::LayerMode;
        let mut here = Document::default();
        let mine = rect(&mut here, 0.0, 0.0, 10.0, 10.0); // layer 0, which this design uses
        here.layers[0].speed = 111.0;
        let mut other = Document::default();
        let a = rect(&mut other, 50.0, 50.0, 10.0, 10.0);
        let c = other.add_shape(ShapeKind::Rect { width: 5.0, height: 5.0, corner_radius: 0.0 }, Affine::translate((80.0, 80.0)), 2);
        other.group(&[a, c].into());
        other.layers[0].speed = 999.0; // must not override ours
        other.layers[2].mode = LayerMode::Fill; // a layer we don't use: comes across
        other.params.params.push(crate::constraints::Param { name: "w".into(), value: "10".into() });
        use crate::constraints::{Anchor, Axis, Constraint, ConstraintKind};
        other.params.constraints.push(Constraint { kind: ConstraintKind::Size { shape: a, axis: Axis::X, anchor: Anchor::Start }, value: "w".into(), enabled: true });
        let added = merge_project(&mut here, &other);
        assert_eq!(added.len(), 2);
        assert!(here.shape(mine).is_some() && !added.contains(&mine));
        assert_eq!(here.layers[0].speed, 111.0, "our layer settings are kept");
        assert_eq!(here.layers[2].mode, LayerMode::Fill, "settings for a new layer come across");
        let groups: BTreeSet<Option<u64>> = added.iter().map(|id| here.shape(*id).unwrap().group).collect();
        assert_eq!(groups.len(), 1, "still one group");
        assert!(groups.iter().next().unwrap().is_some());
        // The constraint points at the copy, not the original id.
        assert_eq!(here.params.constraints.len(), 1);
        let ConstraintKind::Size { shape, .. } = here.params.constraints[0].kind else { panic!() };
        assert!(added.contains(&shape));
        // Importing again doesn't duplicate the parameter.
        merge_project(&mut here, &other);
        assert_eq!(here.params.params.len(), 1);
    }

    #[test]
    fn align_both_centres() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 100.0, 60.0);
        let b = rect(&mut doc, 70.0, 40.0, 10.0, 10.0);
        align(&mut doc, &[a, b].into(), Align::Centers);
        let (ba, bb) = (super::tests::b(&doc, a), super::tests::b(&doc, b));
        assert!((ba.center() - bb.center()).hypot() < 1e-9, "{ba:?} {bb:?}");
        // Both end up on the centre of the selection as it was (0..100 × 0..60 → 50, 30).
        assert!((bb.center() - Point::new(50.0, 30.0)).hypot() < 1e-9 && (bb.width() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn align_left_and_distribute() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let c = rect(&mut doc, 50.0, 20.0, 10.0, 10.0);
        let m = rect(&mut doc, 40.0, 40.0, 10.0, 10.0);
        let sel = Selection::from([a, c, m]);
        distribute(&mut doc, &sel, true);
        assert!(near(b(&doc, m).center().x, 30.0), "middle centre between 5 and 55");
        align(&mut doc, &sel, Align::Left);
        assert!([a, c, m].iter().all(|&id| near(b(&doc, id).x0, 0.0)));
    }

    #[test]
    fn groups_align_as_one_unit() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let g1 = rect(&mut doc, 20.0, 0.0, 10.0, 10.0);
        let g2 = rect(&mut doc, 40.0, 0.0, 10.0, 10.0);
        doc.group(&Selection::from([g1, g2]));
        align(&mut doc, &Selection::from([a, g1, g2]), Align::Left);
        assert!(near(b(&doc, g1).x0, 0.0) && near(b(&doc, g2).x0, 20.0), "group keeps its internal layout");
    }

    #[test]
    fn resize_rotate_mirror() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 10.0, 10.0, 10.0, 20.0);
        let sel = Selection::from([a]);
        resize(&mut doc, &sel, 30.0, 10.0);
        let r = b(&doc, a);
        assert!(near(r.x0, 10.0) && near(r.y0, 10.0) && near(r.width(), 30.0) && near(r.height(), 10.0));
        rotate(&mut doc, &sel, 90.0);
        let r2 = b(&doc, a);
        assert!(near(r2.width(), 10.0) && near(r2.height(), 30.0) && r2.center().distance(r.center()) < 1e-6);
        mirror(&mut doc, &sel, true);
        assert!(b(&doc, a).center().distance(r.center()) < 1e-6);
    }

    #[test]
    fn grid_and_circular_arrays() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let copies = grid_array(&mut doc, &Selection::from([a]), 3, 2, Vec2::new(5.0, 5.0));
        assert_eq!(copies.len(), 5);
        let all = doc.bounds_of(doc.shapes.iter().map(|s| s.id)).unwrap();
        assert!(near(all.width(), 40.0) && near(all.height(), 25.0));

        let mut doc = Document::default();
        let a = rect(&mut doc, 45.0, 0.0, 10.0, 10.0);
        let copies = circular_array(&mut doc, &Selection::from([a]), 4, Point::new(50.0, 50.0), true);
        assert_eq!(copies.len(), 3);
        let all = doc.bounds_of(doc.shapes.iter().map(|s| s.id)).unwrap();
        assert!(all.center().distance(Point::new(50.0, 50.0)) < 1e-6);
    }

    #[test]
    fn duplicated_groups_get_new_group_ids() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let c = rect(&mut doc, 20.0, 0.0, 10.0, 10.0);
        let g = doc.group(&Selection::from([a, c])).unwrap();
        let copies = duplicate(&mut doc, &Selection::from([a, c]), Affine::translate((0.0, 50.0)));
        let groups: BTreeSet<_> = copies.iter().map(|id| doc.shape(*id).unwrap().group).collect();
        assert_eq!(groups.len(), 1);
        assert_ne!(groups.into_iter().next().unwrap(), Some(g));
    }

    #[test]
    fn boolean_replaces_originals() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let c = rect(&mut doc, 5.0, 0.0, 10.0, 10.0);
        let id = boolean(&mut doc, &Selection::from([a, c]), BooleanOp::Subtract).unwrap();
        assert_eq!(doc.shapes.len(), 1);
        let r = b(&doc, id);
        assert!(near(r.width(), 5.0) && near(r.height(), 10.0));
    }

    #[test]
    fn offset_adds_outline() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let id = offset(&mut doc, &Selection::from([a]), 2.0, 3, false).unwrap();
        assert_eq!(doc.shapes.len(), 2);
        assert_eq!(doc.shape(id).unwrap().layer, 3);
        assert!((b(&doc, id).width() - 14.0).abs() < 1e-3);
    }

    #[test]
    fn nesting_packs_without_overlap() {
        let mut doc = Document::default();
        let ids: Vec<u64> = (0..6).map(|i| rect(&mut doc, 300.0 + i as f64 * 5.0, 300.0, 30.0, 20.0)).collect();
        let tall = rect(&mut doc, 0.0, 300.0, 10.0, 40.0);
        let sel: Selection = ids.iter().copied().chain([tall]).collect();
        let area = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(nest(&mut doc, &sel, area, 2.0, true), 7);
        let boxes: Vec<Rect> = sel.iter().map(|&id| b(&doc, id)).collect();
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.x0 >= -1e-9 && a.y0 >= -1e-9 && a.x1 <= 100.0 + 1e-9 && a.y1 <= 100.0 + 1e-9, "inside area: {a:?}");
            for c in &boxes[i + 1..] {
                assert!(a.intersect(*c).area() < 1e-6, "no overlap: {a:?} {c:?}");
            }
        }
        assert!(b(&doc, tall).width() > b(&doc, tall).height(), "tall part laid flat");
    }

    #[test]
    fn nesting_reports_what_does_not_fit() {
        let mut doc = Document::default();
        let big = rect(&mut doc, 0.0, 0.0, 200.0, 200.0);
        assert_eq!(nest(&mut doc, &Selection::from([big]), Rect::new(0.0, 0.0, 100.0, 100.0), 1.0, true), 0);
    }

    fn line(doc: &mut Document, a: (f64, f64), b: (f64, f64)) -> u64 {
        doc.add_shape(ShapeKind::Path(bc_geom::polyline_path(&[Point::new(a.0, a.1), Point::new(b.0, b.1)], false)), Affine::IDENTITY, 0)
    }

    #[test]
    fn draw_order() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 1.0, 1.0);
        let c = rect(&mut doc, 0.0, 0.0, 1.0, 1.0);
        let d = rect(&mut doc, 0.0, 0.0, 1.0, 1.0);
        let order = |doc: &Document| doc.shapes.iter().map(|s| s.id).collect::<Vec<_>>();
        reorder(&mut doc, &Selection::from([a]), Order::ToFront);
        assert_eq!(order(&doc), vec![c, d, a]);
        reorder(&mut doc, &Selection::from([a]), Order::Backward);
        assert_eq!(order(&doc), vec![c, a, d]);
        reorder(&mut doc, &Selection::from([d]), Order::ToBack);
        assert_eq!(order(&doc), vec![d, c, a]);
        reorder(&mut doc, &Selection::from([d]), Order::Forward);
        assert_eq!(order(&doc), vec![c, d, a]);
    }

    #[test]
    fn break_apart_and_join_back() {
        let mut doc = Document::default();
        let mut two = bc_geom::polyline_path(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], false);
        two.extend(bc_geom::polyline_path(&[Point::new(10.0, 0.0), Point::new(10.0, 10.0)], false));
        let id = doc.add_shape(ShapeKind::Path(two), Affine::IDENTITY, 0);
        let parts = break_apart(&mut doc, &Selection::from([id]));
        assert_eq!(parts.len(), 2);
        assert_eq!(doc.shapes.len(), 2);
        let joined = auto_join(&mut doc, &parts, 0.01);
        assert_eq!(joined.len(), 1);
        assert_eq!(bc_geom::nodes::handles(&doc.shapes[0].local_path()).len(), 3);
    }

    #[test]
    fn auto_join_closes_loops_and_flips_reversed_pieces() {
        let mut doc = Document::default();
        let ids = [line(&mut doc, (0.0, 0.0), (10.0, 0.0)), line(&mut doc, (10.0, 10.0), (10.0, 0.0)), line(&mut doc, (10.0, 10.0), (0.0, 0.0))];
        let joined = auto_join(&mut doc, &ids.into_iter().collect(), 0.01);
        assert_eq!(joined.len(), 1);
        assert!(doc.shapes[0].is_closed());
    }

    #[test]
    fn close_and_reverse() {
        let mut doc = Document::default();
        let id = doc.add_shape(
            ShapeKind::Path(bc_geom::polyline_path(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.05, 0.0)], false)),
            Affine::IDENTITY,
            0,
        );
        assert_eq!(close_paths(&mut doc, &Selection::from([id]), 0.01), 0);
        assert_eq!(close_paths(&mut doc, &Selection::from([id]), 0.1), 1);
        assert!(doc.shape(id).unwrap().is_closed());
        let first = doc.shape(id).unwrap().local_path().elements()[0];
        reverse(&mut doc, &Selection::from([id]));
        assert_ne!(doc.shape(id).unwrap().local_path().elements()[0], first);
    }

    #[test]
    fn same_size_dock_and_duplicates() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 20.0, 10.0);
        let c = rect(&mut doc, 50.0, 50.0, 10.0, 10.0);
        make_same_size(&mut doc, &Selection::from([a, c]), true);
        assert!(near(b(&doc, c).width(), 20.0) && near(b(&doc, c).height(), 20.0));
        dock(&mut doc, &Selection::from([a, c]), Side::Left, 1.0);
        assert!(near(b(&doc, a).x0, 0.0) && near(b(&doc, c).x0, 21.0), "c docks against a: {:?}", b(&doc, c));
        let dup = rect(&mut doc, 0.0, 0.0, 20.0, 10.0);
        assert_eq!(delete_duplicates(&mut doc), 1);
        assert!(doc.shape(dup).is_none());
    }

    #[test]
    fn convert_text_like_shapes_to_paths() {
        let mut doc = Document::default();
        let a = rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let before = b(&doc, a);
        convert_to_path(&mut doc, &Selection::from([a]));
        assert!(matches!(doc.shape(a).unwrap().kind, ShapeKind::Path(_)));
        assert_eq!(b(&doc, a), before);
    }
}

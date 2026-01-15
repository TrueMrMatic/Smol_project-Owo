//! Step 2A tessellation (fills only): convert Ruffle distilled shape paths into
//! a cached triangle mesh in pixel units.
//!
//! Design constraints:
//! - This module is allowed to use Ruffle types.
//! - Output types must be renderer-owned: `Vec<Vertex2>` + `Vec<u16>`.
//! - No per-frame allocations: tessellation runs at **register_shape** time.

use crate::render::cache::shapes::{FillMesh, Vertex2};
use ruffle_render::shape_utils::{DistilledShape, DrawCommand, DrawPath, FillRule};

// We use earcut for robust polygon-with-holes triangulation.
// This runs at registration time, so the CPU cost is acceptable.
use earcutr::earcut;

#[derive(Debug)]
pub enum TessError {
    NoContours,
    TooManyVerts,
    EarcutFailed,
}

/// Output of shape tessellation.
///
/// We keep one mesh per fill path to allow multi-fill rendering (still fills-only).
#[derive(Debug)]
pub struct TessOutput {
    pub fills: Vec<FillMesh>,
    /// True if at least one fill failed to tessellate.
    pub any_failed: bool,
}

/// Tessellate filled regions of a Ruffle distilled shape.
///
/// Output coordinates are in **pixel units**, in the shape's local space.
pub fn tessellate_fills(shape: &DistilledShape<'_>) -> Result<TessOutput, TessError> {
    // Registration-time tessellation.
    // We output one mesh per Fill path so the renderer can draw multiple fills for a single shape.
    // Each Fill path comes with its own winding_rule.
    let mut fills: Vec<FillMesh> = Vec::new();
    let mut any_failed = false;

    for path in &shape.paths {
        let (commands, rule) = match path {
            DrawPath::Fill { commands, winding_rule, .. } => (commands, *winding_rule),
            _ => continue, // fills-only Step 2A
        };

        let mut out_verts: Vec<Vertex2> = Vec::new();
        let mut out_indices: Vec<u16> = Vec::new();

        // 1) Flatten commands into closed contours (multiple subpaths supported).
        let mut contours: Vec<Vec<(f32, f32)>> = flatten_commands_to_contours(commands.iter(), 0.75);
        for c in contours.iter_mut() {
            normalize_ring(c);
            simplify_ring(c);
        }
        contours.retain(|c| c.len() >= 3 && polygon_area_abs(c) > 0.5);
        if contours.is_empty() {
            any_failed = true;
            continue;
        }

        // 2) Group contours into outer-with-holes based on fill rule.
        let groups = group_contours_more_correct(&contours, rule);
        if groups.is_empty() {
            any_failed = true;
            continue;
        }

        // 3) Triangulate each outer-with-holes group using earcut and merge into this fill mesh.
        for group in groups {
            let base = out_verts.len();
            if base >= u16::MAX as usize {
                return Err(TessError::TooManyVerts);
            }

            let mut coords: Vec<f64> = Vec::new();
            let mut hole_starts: Vec<usize> = Vec::new();

            append_contour(&mut coords, &mut out_verts, &group.outer);
            for h in &group.holes {
                hole_starts.push(out_verts.len() - base);
                append_contour(&mut coords, &mut out_verts, h);
            }

            let idx = earcut(&coords, &hole_starts, 2).map_err(|_| TessError::EarcutFailed)?;
            if idx.len() < 3 || idx.len() % 3 != 0 {
                return Err(TessError::EarcutFailed);
            }

            for &i in idx.iter() {
                let vi = base + i;
                if vi > u16::MAX as usize {
                    return Err(TessError::TooManyVerts);
                }
                out_indices.push(vi as u16);
            }
        }

        if out_indices.is_empty() {
            any_failed = true;
            continue;
        }

        fills.push(FillMesh { verts: out_verts, indices: out_indices });
    }

    if fills.is_empty() {
        return Err(TessError::NoContours);
    }
    Ok(TessOutput { fills, any_failed })
}

// -----------------
// Path flattening
// -----------------

/// Flatten one DrawPath into one or more closed contours.
///
/// `tol_px` is the maximum deviation in pixels allowed when flattening curves.
fn flatten_commands_to_contours<'a, I>(cmds: I, tol_px: f32) -> Vec<Vec<(f32, f32)>>
where
    I: IntoIterator<Item = &'a DrawCommand>,
{
    let mut contours: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut pen: Option<(f32, f32)> = None;
    let mut start: Option<(f32, f32)> = None;

    let mut finalize = |cur: &mut Vec<(f32, f32)>, start: &mut Option<(f32, f32)>| {
        if cur.len() >= 3 {
            // Ensure closed.
            if let Some(s) = *start {
                let last = cur[cur.len() - 1];
                if (last.0 - s.0).abs() > 0.01 || (last.1 - s.1).abs() > 0.01 {
                    cur.push(s);
                }
            }
            contours.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
        *start = None;
    };

    for cmd in cmds.into_iter() {
        match cmd {
            DrawCommand::MoveTo(p) => {
                finalize(&mut cur, &mut start);
                let pt = (p.x.to_pixels() as f32, p.y.to_pixels() as f32);
                cur.push(pt);
                pen = Some(pt);
                start = Some(pt);
            }
            DrawCommand::LineTo(p) => {
                let pt = (p.x.to_pixels() as f32, p.y.to_pixels() as f32);
                cur.push(pt);
                pen = Some(pt);
            }
            DrawCommand::QuadraticCurveTo { control, anchor } => {
                if let Some(p0) = pen {
                    let p1 = (control.x.to_pixels() as f32, control.y.to_pixels() as f32);
                    let p2 = (anchor.x.to_pixels() as f32, anchor.y.to_pixels() as f32);
                    flatten_quad(p0, p1, p2, tol_px, 0, &mut cur);
                    pen = Some(p2);
                }
            }
            DrawCommand::CubicCurveTo { control_a, control_b, anchor } => {
                if let Some(p0) = pen {
                    let p1 = (control_a.x.to_pixels() as f32, control_a.y.to_pixels() as f32);
                    let p2 = (control_b.x.to_pixels() as f32, control_b.y.to_pixels() as f32);
                    let p3 = (anchor.x.to_pixels() as f32, anchor.y.to_pixels() as f32);
                    flatten_cubic(p0, p1, p2, p3, tol_px, 0, &mut cur);
                    pen = Some(p3);
                }
            }
        }
    }

    finalize(&mut cur, &mut start);
    contours
}

#[inline(always)]
fn dist_point_to_line(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let vx = bx - ax;
    let vy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let c1 = vx * wx + vy * wy;
    if c1 <= 0.0 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let c2 = vx * vx + vy * vy;
    if c2 <= c1 {
        return ((px - bx).powi(2) + (py - by).powi(2)).sqrt();
    }
    let t = c1 / c2;
    let proj = (ax + t * vx, ay + t * vy);
    ((px - proj.0).powi(2) + (py - proj.1).powi(2)).sqrt()
}

fn flatten_quad(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), tol: f32, depth: u32, out: &mut Vec<(f32, f32)>) {
    if depth >= 10 {
        out.push(p2);
        return;
    }
    // Deviation is distance of control to baseline.
    let d = dist_point_to_line(p1, p0, p2);
    if d <= tol {
        out.push(p2);
        return;
    }
    // Subdivide at t=0.5 via De Casteljau.
    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p012 = midpoint(p01, p12);
    flatten_quad(p0, p01, p012, tol, depth + 1, out);
    flatten_quad(p012, p12, p2, tol, depth + 1, out);
}

fn flatten_cubic(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), tol: f32, depth: u32, out: &mut Vec<(f32, f32)>) {
    if depth >= 10 {
        out.push(p3);
        return;
    }
    // Use max distance of both controls to baseline as error metric.
    let d1 = dist_point_to_line(p1, p0, p3);
    let d2 = dist_point_to_line(p2, p0, p3);
    if d1.max(d2) <= tol {
        out.push(p3);
        return;
    }
    // Subdivide at t=0.5 via De Casteljau.
    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p23 = midpoint(p2, p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let p0123 = midpoint(p012, p123);
    flatten_cubic(p0, p01, p012, p0123, tol, depth + 1, out);
    flatten_cubic(p0123, p123, p23, p3, tol, depth + 1, out);
}

#[inline(always)]
fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

#[inline(always)]
fn approx_eq(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

/// Remove a duplicated closing vertex if present.
///
/// Many Fill paths already end with the start point; earcut and our containment tests
/// are happier when rings are not explicitly closed.
fn normalize_ring(ring: &mut Vec<(f32, f32)>) {
    while ring.len() >= 2 && approx_eq(ring[0], ring[ring.len() - 1]) {
        ring.pop();
    }
}

/// Light cleanup to help earcut succeed on noisy contours.
///
/// - drops consecutive duplicates / near-duplicates
/// - drops near-collinear points
fn simplify_ring(ring: &mut Vec<(f32, f32)>) {
    if ring.len() < 3 {
        return;
    }

    // 1) Consecutive duplicate cull
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(ring.len());
    for &p in ring.iter() {
        if out.last().copied().map(|q| (p.0 - q.0).abs() < 0.05 && (p.1 - q.1).abs() < 0.05).unwrap_or(false) {
            continue;
        }
        out.push(p);
    }
    if out.len() >= 2 && (out[0].0 - out[out.len() - 1].0).abs() < 0.05 && (out[0].1 - out[out.len() - 1].1).abs() < 0.05 {
        out.pop();
    }

    // 2) Collinear cull
    if out.len() >= 3 {
        let mut out2: Vec<(f32, f32)> = Vec::with_capacity(out.len());
        for i in 0..out.len() {
            let prev = out[(i + out.len() - 1) % out.len()];
            let cur = out[i];
            let next = out[(i + 1) % out.len()];
            let ax = cur.0 - prev.0;
            let ay = cur.1 - prev.1;
            let bx = next.0 - cur.0;
            let by = next.1 - cur.1;
            let cross = ax * by - ay * bx;
            if cross.abs() < 0.05 {
                continue;
            }
            out2.push(cur);
        }
        if out2.len() >= 3 {
            *ring = out2;
            return;
        }
    }
    *ring = out;
}

/// Pick a point that is (very likely) just inside the contour.
fn sample_point_inside_contour(contour: &[(f32, f32)]) -> (f32, f32) {
    // Find a non-degenerate edge.
    let mut p0 = contour[0];
    let mut p1 = contour[1];
    for w in contour.windows(2) {
        let a = w[0];
        let b = w[1];
        if (a.0 - b.0).abs() + (a.1 - b.1).abs() > 1e-3 {
            p0 = a;
            p1 = b;
            break;
        }
    }
    let dx = p1.0 - p0.0;
    let dy = p1.1 - p0.1;
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let nx = -dy / len;
    let ny = dx / len;
    let eps = 0.2;
    let c1 = (p0.0 + nx * eps, p0.1 + ny * eps);
    let c2 = (p0.0 - nx * eps, p0.1 - ny * eps);
    if point_in_poly(c1, contour) {
        return c1;
    }
    if point_in_poly(c2, contour) {
        return c2;
    }
    // Fallback: centroid-ish.
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    for &(x, y) in contour {
        cx += x;
        cy += y;
    }
    (cx / (contour.len() as f32), cy / (contour.len() as f32))
}

/// Evaluate fill rule at point `p` considering all contours.
fn filled_at_point(p: (f32, f32), contours: &[Vec<(f32, f32)>], rule: FillRule) -> bool {
    match rule {
        FillRule::EvenOdd => {
            let mut inside = false;
            for c in contours {
                if point_in_poly(p, c) {
                    inside = !inside;
                }
            }
            inside
        }
        FillRule::NonZero => {
            let mut wn: i32 = 0;
            for c in contours {
                wn += winding_number(p, c);
            }
            wn != 0
        }
    }
}

fn winding_number(p: (f32, f32), poly: &[(f32, f32)]) -> i32 {
    // Classic winding number algorithm.
    let (px, py) = p;
    let mut wn: i32 = 0;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (x0, y0) = poly[j];
        let (x1, y1) = poly[i];
        if y0 <= py {
            if y1 > py && is_left((x0, y0), (x1, y1), (px, py)) > 0.0 {
                wn += 1;
            }
        } else if y1 <= py && is_left((x0, y0), (x1, y1), (px, py)) < 0.0 {
            wn -= 1;
        }
        j = i;
    }
    wn
}

#[inline(always)]
fn is_left(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (p.0 - a.0) * (b.1 - a.1)
}

// -----------------
// Hole handling
// -----------------

#[derive(Clone, Debug)]
struct ContourGroup {
    outer: Vec<(f32, f32)>,
    holes: Vec<Vec<(f32, f32)>>,
}

fn group_contours_more_correct(contours: &[Vec<(f32, f32)>], rule: FillRule) -> Vec<ContourGroup> {
    // Compute bbox for each contour.
    let mut bbox: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(contours.len());
    for c in contours {
        bbox.push(poly_bbox(c));
    }

    // Build parent relation: smallest contour that contains this one.
    let mut parent: Vec<Option<usize>> = vec![None; contours.len()];
    for i in 0..contours.len() {
        // Use a point guaranteed to be inside the contour for containment tests.
        let p = sample_point_inside_contour(&contours[i]);
        let mut best: Option<usize> = None;
        let mut best_area = f32::INFINITY;
        for j in 0..contours.len() {
            if i == j { continue; }
            if !bbox_contains(bbox[j], p) { continue; }
            if !point_in_poly(p, &contours[j]) { continue; }
            let a = polygon_area_abs(&contours[j]);
            if a < best_area {
                best_area = a;
                best = Some(j);
            }
        }
        parent[i] = best;
    }

    // Classify each contour as an "outer boundary" vs a "hole boundary" by sampling a point
    // just inside the contour and evaluating the fill rule against *all* contours.
    //
    // This is more robust than using depth parity or winding sign heuristics, and handles
    // nested holes (hole-in-hole) for both EvenOdd and NonZero rules.
    let mut is_outer: Vec<bool> = vec![false; contours.len()];
    for i in 0..contours.len() {
        let p = sample_point_inside_contour(&contours[i]);
        is_outer[i] = filled_at_point(p, contours, rule);
    }

    let mut groups: Vec<ContourGroup> = Vec::new();
    let mut outer_map: Vec<Option<usize>> = vec![None; contours.len()];
    for i in 0..contours.len() {
        if is_outer[i] {
            outer_map[i] = Some(groups.len());
            groups.push(ContourGroup { outer: contours[i].clone(), holes: Vec::new() });
        }
    }

    // Assign hole contours to the nearest outer ancestor.
    for i in 0..contours.len() {
        if is_outer[i] {
            continue;
        }
        let mut cur = parent[i];
        while let Some(p) = cur {
            if let Some(g) = outer_map[p] {
                groups[g].holes.push(contours[i].clone());
                break;
            }
            cur = parent[p];
        }
    }

    groups
}

#[inline(always)]
fn poly_bbox(c: &[(f32, f32)]) -> (f32, f32, f32, f32) {
    let mut minx = c[0].0;
    let mut maxx = c[0].0;
    let mut miny = c[0].1;
    let mut maxy = c[0].1;
    for &(x, y) in c.iter().skip(1) {
        minx = minx.min(x);
        maxx = maxx.max(x);
        miny = miny.min(y);
        maxy = maxy.max(y);
    }
    (minx, miny, maxx, maxy)
}

#[inline(always)]
fn bbox_contains(bb: (f32, f32, f32, f32), p: (f32, f32)) -> bool {
    p.0 > bb.0 && p.0 < bb.2 && p.1 > bb.1 && p.1 < bb.3
}

fn point_in_poly(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    // Ray casting.
    let (px, py) = p;
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let intersect = ((yi > py) != (yj > py))
            && (px < (xj - xi) * (py - yi) / (yj - yi + 1e-12) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[inline(always)]
fn polygon_area_signed(poly: &[(f32, f32)]) -> f32 {
    // Shoelace. (Note: screen coords are y-down; sign is still usable for relative winding.)
    let mut a = 0.0f32;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        a += (poly[j].0 * poly[i].1) - (poly[i].0 * poly[j].1);
        j = i;
    }
    0.5 * a
}

#[inline(always)]
fn polygon_area_abs(poly: &[(f32, f32)]) -> f32 {
    polygon_area_signed(poly).abs()
}

fn append_contour(coords: &mut Vec<f64>, out_verts: &mut Vec<Vertex2>, contour: &[(f32, f32)]) {
    // Drop the duplicated closing vertex if present, because earcut expects simple rings.
    let mut n = contour.len();
    if n >= 2 {
        let first = contour[0];
        let last = contour[n - 1];
        if (first.0 - last.0).abs() < 0.01 && (first.1 - last.1).abs() < 0.01 {
            n -= 1;
        }
    }
    for &(x, y) in contour.iter().take(n) {
        coords.push(x as f64);
        coords.push(y as f64);
        out_verts.push(Vertex2 { x: x.round() as i32, y: y.round() as i32 });
    }
}

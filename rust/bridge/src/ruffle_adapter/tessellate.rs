//! Step 2A tessellation (fills only): convert Ruffle distilled shape paths into
//! a cached triangle mesh in pixel units.
//!
//! Design constraints:
//! - This module is allowed to use Ruffle types.
//! - Output types must be renderer-owned: `Vec<Vertex2>` + `Vec<u16>`.
//! - No per-frame allocations: tessellation runs at **register_shape** time.

use crate::render::cache::shapes::{FillMesh, FillPaint, StrokeMesh, Vertex2};
use crate::runlog;
use ruffle_render::shape_utils::{DistilledShape, DrawCommand, DrawPath, FillRule};
use ruffle_core::swf::{FillStyle, LineJoinStyle};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

// We use earcut for robust polygon-with-holes triangulation.
// This runs at registration time, so the CPU cost is acceptable.
use earcutr::earcut;

#[derive(Debug)]
pub enum TessError {
    NoContours,
    TooManyVerts,
    EarcutFailed,
    EarcutDenied,
    Timeout,
}

const MAX_POINTS_PER_FILL: usize = 4096;
const MAX_POINTS_PER_STROKE: usize = 4096;
const MAX_VERTS_PER_MESH: usize = u16::MAX as usize;
const MAX_CONTOURS_PER_FILL: usize = 64;
const MAX_TOTAL_CONTAINMENT_TESTS: usize = 4096;
const FILL_PATH_BUDGET_MS: u64 = 60;
const MAX_UNSUPPORTED_FILL_WARNINGS: u32 = 8;
const EARCUT_MAX_TOTAL_POINTS: usize = 256;
const EARCUT_MAX_HOLES: usize = 8;
const EARCUT_MAX_OUTER_POINTS: usize = 192;
const CONVEX_FAN_MAX_OUTER_POINTS: usize = 128;

static UNSUPPORTED_FILL_WARNINGS: AtomicU32 = AtomicU32::new(0);

type Point = (f32, f32);

/// Output of shape tessellation.
///
/// We keep one mesh per fill path to allow multi-fill rendering (still fills-only).
#[derive(Debug)]
pub struct TessOutput {
    pub fills: Vec<FillMesh>,
    /// True if at least one fill failed to tessellate.
    pub any_failed: bool,
    pub group_used_more_correct: u32,
    pub group_used_fast: u32,
    pub group_used_trivial: u32,
    pub unsupported_fill_paints: u32,
}

#[derive(Debug)]
pub struct StrokeOutput {
    pub strokes: Vec<StrokeMesh>,
    pub any_failed: bool,
}

/// Tessellate filled regions of a Ruffle distilled shape.
///
/// Output coordinates are in **pixel units**, in the shape's local space.
pub fn tessellate_fills(shape: &DistilledShape<'_>, shape_id: u32) -> Result<TessOutput, TessError> {
    // Registration-time tessellation.
    // We output one mesh per Fill path so the renderer can draw multiple fills for a single shape.
    // Each Fill path comes with its own winding_rule.
    let mut fills: Vec<FillMesh> = Vec::new();
    let mut any_failed = false;
    let mut fill_paths = 0usize;
    let mut group_used_more_correct: u32 = 0;
    let mut group_used_fast: u32 = 0;
    let mut group_used_trivial: u32 = 0;
    let mut unsupported_fill_paints: u32 = 0;
    let mut logged_cap_contours = false;
    let mut logged_cap_tests = false;
    let mut logged_timeout = false;
    let mut logged_convex_fan = false;

    let tol_px = tessellation_tolerance_px(shape);
    for path in &shape.paths {
        let fill_idx = fill_paths.saturating_add(1);
        let (commands, rule, paint) = match path {
            DrawPath::Fill { commands, winding_rule, style, .. } => {
                let paint = match style {
                    FillStyle::Color(color) => FillPaint::SolidRGBA(color.r, color.g, color.b, color.a),
                    _ => {
                        unsupported_fill_paints = unsupported_fill_paints.saturating_add(1);
                        let count = UNSUPPORTED_FILL_WARNINGS.fetch_add(1, Ordering::Relaxed);
                        if count < MAX_UNSUPPORTED_FILL_WARNINGS {
                            runlog::warn_line(&format!(
                                "fill_style unsupported shape={} fill_path={}",
                                shape_id, fill_idx
                            ));
                        }
                        FillPaint::Unsupported
                    }
                };
                (commands, *winding_rule, paint)
            }
            _ => continue, // fills-only Step 2A
        };
        fill_paths = fill_idx;
        let fill_start = Instant::now();

        let mut out_verts: Vec<Vertex2> = Vec::new();
        let mut out_indices: Vec<u16> = Vec::new();

        // 1) Flatten commands into closed contours (multiple subpaths supported).
        let mut contours: Vec<Vec<(f32, f32)>> = flatten_commands_to_contours(commands.iter(), tol_px);
        for c in contours.iter_mut() {
            normalize_ring(c);
            simplify_ring(c);
        }
        contours.retain(|c| c.len() >= 3 && polygon_area_abs(c) > 0.5);
        if contours.is_empty() {
            any_failed = true;
            continue;
        }
        let contour_count = contours.len();
        let total_points: usize = contours.iter().map(|c| c.len()).sum();
        if contour_count > MAX_CONTOURS_PER_FILL {
            any_failed = true;
            if !logged_cap_contours {
                logged_cap_contours = true;
                runlog::warn_line(&format!(
                    "tess_guard cap_contours shape={} contours={} points={}",
                    shape_id, contour_count, total_points
                ));
            }
            continue;
        }
        if total_points > MAX_POINTS_PER_FILL {
            any_failed = true;
            runlog::warn_line(&format!(
                "tessellate_fills cap_points shape={} total={} paths={}",
                shape_id, total_points, fill_paths
            ));
            continue;
        }
        if fill_start.elapsed().as_millis() as u64 > FILL_PATH_BUDGET_MS {
            any_failed = true;
            if !logged_timeout {
                logged_timeout = true;
                runlog::warn_line(&format!(
                    "tess_guard timeout shape={} contours={} points={}",
                    shape_id, contour_count, total_points
                ));
            }
            continue;
        }

        // 2) Group contours into outer-with-holes based on fill rule.
        let groups = if contour_count <= 16 && total_points <= 800 {
            match group_contours_more_correct(
                &contours,
                rule,
                &fill_start,
                FILL_PATH_BUDGET_MS,
            ) {
                GroupContoursResult::Groups(groups) => {
                    group_used_more_correct = group_used_more_correct.saturating_add(1);
                    groups
                }
                GroupContoursResult::CapTests => {
                    if !logged_cap_tests {
                        logged_cap_tests = true;
                        runlog::warn_line(&format!(
                            "tess_guard cap_tests shape={} contours={} points={} max_tests={}",
                            shape_id, contour_count, total_points, MAX_TOTAL_CONTAINMENT_TESTS
                        ));
                    }
                    runlog::warn_line(&format!(
                        "tess_group fallback=fast shape={} contours={} points={}",
                        shape_id, contour_count, total_points
                    ));
                    match group_contours_fast_parent_depth(
                        &contours,
                        rule,
                        &fill_start,
                        FILL_PATH_BUDGET_MS,
                    ) {
                        GroupContoursResult::Groups(groups) => {
                            group_used_fast = group_used_fast.saturating_add(1);
                            groups
                        }
                        GroupContoursResult::CapTests => {
                            if !logged_cap_tests {
                                logged_cap_tests = true;
                                runlog::warn_line(&format!(
                                    "tess_guard cap_tests shape={} contours={} points={} max_tests={}",
                                    shape_id, contour_count, total_points, MAX_TOTAL_CONTAINMENT_TESTS
                                ));
                            }
                            runlog::warn_line(&format!(
                                "tess_group fallback=trivial shape={} contours={} points={}",
                                shape_id, contour_count, total_points
                            ));
                            group_used_trivial = group_used_trivial.saturating_add(1);
                            group_contours_trivial(&contours)
                        }
                        GroupContoursResult::Timeout => {
                            if !logged_timeout {
                                logged_timeout = true;
                                runlog::warn_line(&format!(
                                    "tess_guard timeout shape={} contours={} points={}",
                                    shape_id, contour_count, total_points
                                ));
                            }
                            runlog::warn_line(&format!(
                                "tess_group fallback=trivial shape={} contours={} points={}",
                                shape_id, contour_count, total_points
                            ));
                            group_used_trivial = group_used_trivial.saturating_add(1);
                            group_contours_trivial(&contours)
                        }
                    }
                }
                GroupContoursResult::Timeout => {
                    if !logged_timeout {
                        logged_timeout = true;
                        runlog::warn_line(&format!(
                            "tess_guard timeout shape={} contours={} points={}",
                            shape_id, contour_count, total_points
                        ));
                    }
                    runlog::warn_line(&format!(
                        "tess_group fallback=fast shape={} contours={} points={}",
                        shape_id, contour_count, total_points
                    ));
                    match group_contours_fast_parent_depth(
                        &contours,
                        rule,
                        &fill_start,
                        FILL_PATH_BUDGET_MS,
                    ) {
                        GroupContoursResult::Groups(groups) => {
                            group_used_fast = group_used_fast.saturating_add(1);
                            groups
                        }
                        GroupContoursResult::CapTests => {
                            if !logged_cap_tests {
                                logged_cap_tests = true;
                                runlog::warn_line(&format!(
                                    "tess_guard cap_tests shape={} contours={} points={} max_tests={}",
                                    shape_id, contour_count, total_points, MAX_TOTAL_CONTAINMENT_TESTS
                                ));
                            }
                            runlog::warn_line(&format!(
                                "tess_group fallback=trivial shape={} contours={} points={}",
                                shape_id, contour_count, total_points
                            ));
                            group_used_trivial = group_used_trivial.saturating_add(1);
                            group_contours_trivial(&contours)
                        }
                        GroupContoursResult::Timeout => {
                            if !logged_timeout {
                                logged_timeout = true;
                                runlog::warn_line(&format!(
                                    "tess_guard timeout shape={} contours={} points={}",
                                    shape_id, contour_count, total_points
                                ));
                            }
                            runlog::warn_line(&format!(
                                "tess_group fallback=trivial shape={} contours={} points={}",
                                shape_id, contour_count, total_points
                            ));
                            group_used_trivial = group_used_trivial.saturating_add(1);
                            group_contours_trivial(&contours)
                        }
                    }
                }
            }
        } else {
            match group_contours_fast_parent_depth(&contours, rule, &fill_start, FILL_PATH_BUDGET_MS) {
                GroupContoursResult::Groups(groups) => {
                    group_used_fast = group_used_fast.saturating_add(1);
                    groups
                }
                GroupContoursResult::CapTests => {
                    if !logged_cap_tests {
                        logged_cap_tests = true;
                        runlog::warn_line(&format!(
                            "tess_guard cap_tests shape={} contours={} points={} max_tests={}",
                            shape_id, contour_count, total_points, MAX_TOTAL_CONTAINMENT_TESTS
                        ));
                    }
                    runlog::warn_line(&format!(
                        "tess_group fallback=trivial shape={} contours={} points={}",
                        shape_id, contour_count, total_points
                    ));
                    group_used_trivial = group_used_trivial.saturating_add(1);
                    group_contours_trivial(&contours)
                }
                GroupContoursResult::Timeout => {
                    if !logged_timeout {
                        logged_timeout = true;
                        runlog::warn_line(&format!(
                            "tess_guard timeout shape={} contours={} points={}",
                            shape_id, contour_count, total_points
                        ));
                    }
                    runlog::warn_line(&format!(
                        "tess_group fallback=trivial shape={} contours={} points={}",
                        shape_id, contour_count, total_points
                    ));
                    group_used_trivial = group_used_trivial.saturating_add(1);
                    group_contours_trivial(&contours)
                }
            }
        };
        if groups.is_empty() {
            any_failed = true;
            continue;
        }

        // 3) Triangulate each outer-with-holes group using earcut and merge into this fill mesh.
        let mut timed_out = false;
        for mut group in groups {
            if fill_start.elapsed().as_millis() as u64 > FILL_PATH_BUDGET_MS {
                let (group_pts, outer_pts, _hole_pts) = group_point_counts(&group);
                #[cfg(feature = "verbose_logs")]
                runlog::log_important(&format!(
                    "earcut_skip timeout shape={} total_pts={} holes={} outer_pts={}",
                    shape_id,
                    group_pts,
                    group.holes.len(),
                    outer_pts
                ));
                timed_out = true;
                break;
            }
            orient_group_winding(&mut group);
            let base = out_verts.len();
            if base >= MAX_VERTS_PER_MESH {
                runlog::warn_line(&format!(
                    "tessellate_fills too_many_verts shape={} base={} paths={}",
                    shape_id, base, fill_paths
                ));
                return Err(TessError::TooManyVerts);
            }

            let (group_pts, outer_pts, _hole_pts) = group_point_counts(&group);
            let holes = group.holes.len();
            if holes == 0
                && outer_pts >= 3
                && outer_pts <= CONVEX_FAN_MAX_OUTER_POINTS
                && is_convex_ring(&group.outer, CONVEX_FAN_MAX_OUTER_POINTS)
            {
                let ring_len = append_contour_vertices(&mut out_verts, &group.outer);
                if base + ring_len > MAX_VERTS_PER_MESH {
                    runlog::warn_line(&format!(
                        "tessellate_fills too_many_verts shape={} verts={} paths={}",
                        shape_id,
                        out_verts.len(),
                        fill_paths
                    ));
                    return Err(TessError::TooManyVerts);
                }
                triangulate_convex_fan(base, ring_len, &mut out_indices);
                if !logged_convex_fan {
                    logged_convex_fan = true;
                    #[cfg(feature = "verbose_logs")]
                    runlog::log_important(&format!(
                        "triangulate_convex_fan shape={} pts={}",
                        shape_id,
                        outer_pts
                    ));
                }
                continue;
            }

            let mut coords: Vec<f64> = Vec::new();
            let mut hole_starts: Vec<usize> = Vec::new();

            let sanitized_outer = sanitize_ring_for_earcut(&group.outer);
            if sanitized_outer.len() < 3 {
                runlog::warn_line(&format!(
                    "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason=degenerate_ring",
                    shape_id,
                    group_pts,
                    holes,
                    outer_pts
                ));
                #[cfg(feature = "verbose_logs")]
                runlog::stage(
                    &format!(
                        "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason=degenerate_ring",
                        shape_id,
                        group_pts,
                        holes,
                        outer_pts
                    ),
                    0,
                );
                return Err(TessError::EarcutDenied);
            }

            let mut sanitized_holes: Vec<Vec<Point>> = Vec::with_capacity(group.holes.len());
            for h in &group.holes {
                let sanitized = sanitize_ring_for_earcut(h);
                if sanitized.len() < 3 {
                    runlog::warn_line(&format!(
                        "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason=degenerate_ring",
                        shape_id,
                        group_pts,
                        holes,
                        outer_pts
                    ));
                    #[cfg(feature = "verbose_logs")]
                    runlog::stage(
                        &format!(
                            "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason=degenerate_ring",
                            shape_id,
                            group_pts,
                            holes,
                            outer_pts
                        ),
                        0,
                    );
                    return Err(TessError::EarcutDenied);
                }
                sanitized_holes.push(sanitized);
            }

            let (group_pts, outer_pts, _hole_pts) = {
                let hole_pts: usize = sanitized_holes.iter().map(|hole| hole.len()).sum();
                let total_pts = sanitized_outer.len() + hole_pts;
                (total_pts, sanitized_outer.len(), hole_pts)
            };
            let holes = sanitized_holes.len();

            let area = polygon_area_signed_f64(&sanitized_outer).abs();
            if area < 0.5 {
                runlog::warn_line(&format!(
                    "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason=degenerate_area",
                    shape_id,
                    group_pts,
                    holes,
                    outer_pts
                ));
                #[cfg(feature = "verbose_logs")]
                runlog::stage(
                    &format!(
                        "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason=degenerate_area",
                        shape_id,
                        group_pts,
                        holes,
                        outer_pts
                    ),
                    0,
                );
                return Err(TessError::EarcutDenied);
            }

            if let Err(reason) = earcut_allowed(group_pts, outer_pts, holes) {
                runlog::warn_line(&format!(
                    "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason={}",
                    shape_id,
                    group_pts,
                    holes,
                    outer_pts,
                    reason
                ));
                #[cfg(feature = "verbose_logs")]
                runlog::stage(
                    &format!(
                        "earcut_skip shape={} total_pts={} holes={} outer_pts={} reason={}",
                        shape_id,
                        group_pts,
                        holes,
                        outer_pts,
                        reason
                    ),
                    0,
                );
                return Err(TessError::EarcutDenied);
            }

            append_contour(&mut coords, &mut out_verts, &sanitized_outer);
            for h in &sanitized_holes {
                hole_starts.push(out_verts.len() - base);
                append_contour(&mut coords, &mut out_verts, h);
            }
            if out_verts.len() > MAX_VERTS_PER_MESH {
                runlog::warn_line(&format!(
                    "tessellate_fills too_many_verts shape={} verts={} paths={}",
                    shape_id,
                    out_verts.len(),
                    fill_paths
                ));
                return Err(TessError::TooManyVerts);
            }
            if fill_start.elapsed().as_millis() as u64 > FILL_PATH_BUDGET_MS {
                let (group_pts, outer_pts, _hole_pts) = group_point_counts(&group);
                #[cfg(feature = "verbose_logs")]
                runlog::log_important(&format!(
                    "earcut_skip timeout shape={} total_pts={} holes={} outer_pts={}",
                    shape_id,
                    group_pts,
                    group.holes.len(),
                    outer_pts
                ));
                timed_out = true;
                break;
            }

            #[cfg(feature = "verbose_logs")]
            runlog::stage(
                &format!(
                    "earcut_input shape={} pts={} holes={}",
                    shape_id,
                    group_pts,
                    holes
                ),
                0,
            );
            #[cfg(feature = "verbose_logs")]
            runlog::log_important(&format!(
                "earcut_input shape={} total_pts={} holes={} outer_pts={}",
                shape_id,
                group_pts,
                holes,
                outer_pts
            ));
            let idx = earcut(&coords, &hole_starts, 2).map_err(|_| {
                runlog::warn_line(&format!(
                    "tessellate_fills earcut_failed shape={} verts={} holes={} paths={}",
                    shape_id,
                    out_verts.len(),
                    hole_starts.len(),
                    fill_paths
                ));
                TessError::EarcutFailed
            })?;
            #[cfg(feature = "verbose_logs")]
            runlog::log_important(&format!(
                "earcut_done shape={} tris={}",
                shape_id,
                idx.len() / 3
            ));
            if idx.len() < 3 || idx.len() % 3 != 0 {
                runlog::warn_line(&format!(
                    "tessellate_fills earcut_invalid shape={} tris={} paths={}",
                    shape_id,
                    idx.len() / 3,
                    fill_paths
                ));
                return Err(TessError::EarcutFailed);
            }

            for &i in idx.iter() {
                let vi = base + i;
                if vi >= MAX_VERTS_PER_MESH {
                    runlog::warn_line(&format!(
                        "tessellate_fills too_many_verts shape={} idx={} paths={}",
                        shape_id, vi, fill_paths
                    ));
                    return Err(TessError::TooManyVerts);
                }
                out_indices.push(vi as u16);
            }
        }

        if timed_out {
            any_failed = true;
            if !logged_timeout {
                logged_timeout = true;
                runlog::warn_line(&format!(
                    "tess_guard timeout shape={} contours={} points={}",
                    shape_id, contour_count, total_points
                ));
            }
            continue;
        }

        if out_indices.is_empty() {
            any_failed = true;
            continue;
        }

        fills.push(FillMesh { verts: out_verts, indices: out_indices, paint });
    }

    if fills.is_empty() {
        if fill_paths == 0 {
            runlog::warn_line(&format!(
                "tessellate_fills no_fill_paths shape={}",
                shape_id
            ));
        } else {
            runlog::warn_line(&format!(
                "tessellate_fills no_contours shape={} paths={}",
                shape_id, fill_paths
            ));
        }
        return Err(TessError::NoContours);
    }
    Ok(TessOutput {
        fills,
        any_failed,
        group_used_more_correct,
        group_used_fast,
        group_used_trivial,
        unsupported_fill_paints,
    })
}

pub fn tessellate_strokes(shape: &DistilledShape<'_>, shape_id: u32) -> Result<StrokeOutput, TessError> {
    let mut strokes: Vec<StrokeMesh> = Vec::new();
    let mut any_failed = false;
    let tol_px = tessellation_tolerance_px(shape);
    let mut stroke_paths = 0usize;

    for path in &shape.paths {
        let (style, is_closed, commands) = match path {
            DrawPath::Stroke { style, is_closed, commands } => (style, *is_closed, commands),
            _ => continue,
        };
        stroke_paths = stroke_paths.saturating_add(1);

        let FillStyle::Color(color) = style.fill_style() else {
            any_failed = true;
            runlog::warn_line("stroke_tess: non-color stroke fill unsupported");
            continue;
        };

        let width_px = style.width().to_pixels() as f32;
        if width_px <= 0.0 {
            continue;
        }
        let half_w = (width_px * 0.5).max(0.5);
        let miter_limit = match style.join_style() {
            LineJoinStyle::Miter(limit) => f32::from(limit).max(1.0),
            _ => 4.0,
        };

        let mut polylines = flatten_commands_to_polylines(commands.iter(), tol_px, is_closed);
        for line in polylines.iter_mut() {
            simplify_polyline(line);
        }
        polylines.retain(|line| line.len() >= 2);

        let total_points: usize = polylines.iter().map(|c| c.len()).sum();
        if total_points > MAX_POINTS_PER_STROKE {
            any_failed = true;
            runlog::warn_line(&format!(
                "tessellate_strokes cap_points shape={} total={} paths={}",
                shape_id, total_points, stroke_paths
            ));
            continue;
        }

        for line in polylines {
            match build_stroke_mesh(&line, half_w, miter_limit, is_closed) {
                Some(mesh) => {
                    strokes.push(StrokeMesh {
                        verts: mesh.verts,
                        indices: mesh.indices,
                        r: color.r,
                        g: color.g,
                        b: color.b,
                    });
                }
                None => {
                    any_failed = true;
                }
            }
        }
    }

    if strokes.is_empty() {
        if stroke_paths > 0 {
            runlog::warn_line(&format!(
                "tessellate_strokes no_contours shape={} paths={}",
                shape_id, stroke_paths
            ));
        }
        return Err(TessError::NoContours);
    }
    Ok(StrokeOutput { strokes, any_failed })
}

fn tessellation_tolerance_px(_shape: &DistilledShape<'_>) -> f32 {
    0.5
}

// -----------------
// Path flattening
// -----------------

/// Flatten one DrawPath into one or more open polylines.
fn flatten_commands_to_polylines<'a, I>(cmds: I, tol_px: f32, close: bool) -> Vec<Vec<(f32, f32)>>
where
    I: IntoIterator<Item = &'a DrawCommand>,
{
    let mut lines: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut pen: Option<(f32, f32)> = None;
    let mut start: Option<(f32, f32)> = None;

    let mut finalize = |cur: &mut Vec<(f32, f32)>, start: &mut Option<(f32, f32)>| {
        if cur.len() >= 2 {
            if close {
                if let Some(s) = *start {
                    let last = cur[cur.len() - 1];
                    if (last.0 - s.0).abs() > 0.01 || (last.1 - s.1).abs() > 0.01 {
                        cur.push(s);
                    }
                }
            }
            lines.push(std::mem::take(cur));
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
    lines
}

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

fn simplify_polyline(line: &mut Vec<(f32, f32)>) {
    if line.len() < 2 {
        return;
    }
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(line.len());
    for &p in line.iter() {
        if out.last().copied().map(|q| (p.0 - q.0).abs() < 0.05 && (p.1 - q.1).abs() < 0.05).unwrap_or(false) {
            continue;
        }
        out.push(p);
    }
    if out.len() >= 2 {
        *line = out;
    }
}

fn build_stroke_mesh(points: &[(f32, f32)], half_w: f32, miter_limit: f32, closed: bool) -> Option<FillMesh> {
    if points.len() < 2 {
        return None;
    }
    let mut pts = points.to_vec();
    if closed && approx_eq(pts[0], pts[pts.len() - 1]) {
        pts.pop();
    }
    if pts.len() < 2 {
        return None;
    }
    if pts.len() * 2 > MAX_VERTS_PER_MESH {
        return None;
    }

    let count = pts.len();
    let seg_count = if closed { count } else { count - 1 };
    let mut normals: Vec<(f32, f32)> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let p0 = pts[i];
        let p1 = pts[(i + 1) % count];
        let dx = p1.0 - p0.0;
        let dy = p1.1 - p0.1;
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 0.0001 {
            normals.push((0.0, 0.0));
            continue;
        }
        let nx = -dy / len;
        let ny = dx / len;
        normals.push((nx, ny));
    }

    let mut verts: Vec<Vertex2> = Vec::with_capacity(count * 2);
    for i in 0..count {
        let p = pts[i];
        let (n_prev, n_next) = if closed {
            let prev = normals[(i + count - 1) % count];
            let next = normals[i % count];
            (prev, next)
        } else if i == 0 {
            (normals[0], normals[0])
        } else if i == count - 1 {
            (normals[count - 2], normals[count - 2])
        } else {
            (normals[i - 1], normals[i])
        };
        let miter = normalize_vec((n_prev.0 + n_next.0, n_prev.1 + n_next.1));
        let denom = (miter.0 * n_prev.0 + miter.1 * n_prev.1).abs().max(0.0001);
        let mut miter_len = half_w / denom;
        if miter_len > miter_limit * half_w {
            miter_len = half_w;
        }
        let offset = (miter.0 * miter_len, miter.1 * miter_len);
        let left = (p.0 + offset.0, p.1 + offset.1);
        let right = (p.0 - offset.0, p.1 - offset.1);
        verts.push(Vertex2 { x: left.0.round() as i32, y: left.1.round() as i32 });
        verts.push(Vertex2 { x: right.0.round() as i32, y: right.1.round() as i32 });
    }

    let mut indices: Vec<u16> = Vec::new();
    let segs = if closed { count } else { count - 1 };
    for i in 0..segs {
        let next = (i + 1) % count;
        let i0 = (2 * i) as u16;
        let i1 = (2 * i + 1) as u16;
        let i2 = (2 * next) as u16;
        let i3 = (2 * next + 1) as u16;
        indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
    }

    Some(FillMesh { verts, indices, paint: FillPaint::Unsupported })
}

fn normalize_vec(v: (f32, f32)) -> (f32, f32) {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len <= 0.0001 {
        (0.0, 0.0)
    } else {
        (v.0 / len, v.1 / len)
    }
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

fn group_point_counts(group: &ContourGroup) -> (usize, usize, usize) {
    let outer_pts = group.outer.len();
    let hole_pts: usize = group.holes.iter().map(|hole| hole.len()).sum();
    let total_pts = outer_pts + hole_pts;
    (total_pts, outer_pts, hole_pts)
}

fn sanitize_ring_for_earcut(ring: &[Point]) -> Vec<Point> {
    if ring.is_empty() {
        return Vec::new();
    }

    let mut n = ring.len();
    let first = ring[0];
    let last = ring[n - 1];
    if (first.0 - last.0).abs() < 0.01 && (first.1 - last.1).abs() < 0.01 {
        n = n.saturating_sub(1);
    }

    let mut out: Vec<Point> = Vec::with_capacity(n);
    let mut prev_px: Option<(i32, i32)> = None;
    for &(x, y) in ring.iter().take(n) {
        let px = x.round() as i32;
        let py = y.round() as i32;
        if let Some((prev_x, prev_y)) = prev_px {
            if prev_x == px && prev_y == py {
                continue;
            }
        }
        out.push((x, y));
        prev_px = Some((px, py));
    }

    if out.len() >= 2 {
        let first_px = (out[0].0.round() as i32, out[0].1.round() as i32);
        let last_px = (out[out.len() - 1].0.round() as i32, out[out.len() - 1].1.round() as i32);
        if first_px == last_px {
            out.pop();
        }
    }

    out
}

fn is_convex_ring(ring: &[Point], max_pts: usize) -> bool {
    let mut n = ring.len();
    if n < 3 {
        return false;
    }
    let first = ring[0];
    let last = ring[n - 1];
    if (first.0 - last.0).abs() < 0.01 && (first.1 - last.1).abs() < 0.01 {
        n = n.saturating_sub(1);
    }
    if n < 3 || n > max_pts {
        return false;
    }

    let mut sign = 0.0f32;
    let eps = 1.0e-4f32;
    for i in 0..n {
        let prev = ring[(i + n - 1) % n];
        let curr = ring[i];
        let next = ring[(i + 1) % n];
        let cross = (curr.0 - prev.0) * (next.1 - curr.1) - (curr.1 - prev.1) * (next.0 - curr.0);
        if cross.abs() <= eps {
            continue;
        }
        if sign == 0.0 {
            sign = cross;
        } else if sign * cross < 0.0 {
            return false;
        }
    }
    sign != 0.0
}

fn triangulate_convex_fan(base: usize, ring_len: usize, out_indices: &mut Vec<u16>) {
    if ring_len < 3 {
        return;
    }
    for i in 1..(ring_len - 1) {
        out_indices.push(base as u16);
        out_indices.push((base + i) as u16);
        out_indices.push((base + i + 1) as u16);
    }
}

#[inline(always)]
fn polygon_area_signed_f64(poly: &[Point]) -> f64 {
    let mut a = 0.0f64;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xj, yj) = poly[j];
        let (xi, yi) = poly[i];
        a += (xj as f64 * yi as f64) - (xi as f64 * yj as f64);
        j = i;
    }
    0.5 * a
}

fn earcut_allowed(total_pts: usize, outer_pts: usize, holes: usize) -> Result<(), &'static str> {
    if total_pts > EARCUT_MAX_TOTAL_POINTS {
        return Err("total_points");
    }
    if outer_pts > EARCUT_MAX_OUTER_POINTS {
        return Err("outer_points");
    }
    if holes > EARCUT_MAX_HOLES {
        return Err("holes");
    }
    Ok(())
}

enum GroupContoursResult {
    Groups(Vec<ContourGroup>),
    CapTests,
    Timeout,
}

// More-correct grouping: uses fill-rule evaluation to classify outers/holes.
// This is accurate but can be expensive on complex contour sets.
fn group_contours_more_correct(
    contours: &[Vec<(f32, f32)>],
    rule: FillRule,
    start: &Instant,
    budget_ms: u64,
) -> GroupContoursResult {
    // Compute bbox for each contour.
    let mut bbox: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(contours.len());
    for c in contours {
        bbox.push(poly_bbox(c));
    }

    // Build parent relation: smallest contour that contains this one.
    let mut parent: Vec<Option<usize>> = vec![None; contours.len()];
    let mut tests_used: usize = 0;
    for i in 0..contours.len() {
        if start.elapsed().as_millis() as u64 > budget_ms {
            return GroupContoursResult::Timeout;
        }
        // Use a point guaranteed to be inside the contour for containment tests.
        let p = sample_point_inside_contour(&contours[i]);
        let mut best: Option<usize> = None;
        let mut best_area = f32::INFINITY;
        for j in 0..contours.len() {
            if i == j { continue; }
            tests_used = tests_used.saturating_add(1);
            if tests_used > MAX_TOTAL_CONTAINMENT_TESTS {
                return GroupContoursResult::CapTests;
            }
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
        if start.elapsed().as_millis() as u64 > budget_ms {
            return GroupContoursResult::Timeout;
        }
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
        if start.elapsed().as_millis() as u64 > budget_ms {
            return GroupContoursResult::Timeout;
        }
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

    GroupContoursResult::Groups(groups)
}

// Fast grouping: uses parent-depth parity (EvenOdd heuristic) to classify outers/holes.
// For NonZero we intentionally keep the same parity heuristic as a fast approximation.
// If caps/timeouts hit, callers should fall back to trivial grouping.
fn group_contours_fast_parent_depth(
    contours: &[Vec<(f32, f32)>],
    _rule: FillRule,
    start: &Instant,
    budget_ms: u64,
) -> GroupContoursResult {
    // Compute bbox for each contour.
    let mut bbox: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(contours.len());
    for c in contours {
        bbox.push(poly_bbox(c));
    }

    let mut parent: Vec<Option<usize>> = vec![None; contours.len()];
    let mut tests_used: usize = 0;
    for i in 0..contours.len() {
        if start.elapsed().as_millis() as u64 > budget_ms {
            return GroupContoursResult::Timeout;
        }
        let p = sample_point_inside_contour(&contours[i]);
        let mut best: Option<usize> = None;
        let mut best_area = f32::INFINITY;
        for j in 0..contours.len() {
            if i == j { continue; }
            tests_used = tests_used.saturating_add(1);
            if tests_used > MAX_TOTAL_CONTAINMENT_TESTS {
                return GroupContoursResult::CapTests;
            }
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

    let mut depth: Vec<usize> = vec![0; contours.len()];
    for i in 0..contours.len() {
        let mut d = 0usize;
        let mut cur = parent[i];
        while let Some(p) = cur {
            d = d.saturating_add(1);
            cur = parent[p];
        }
        depth[i] = d;
    }

    let mut groups: Vec<ContourGroup> = Vec::new();
    let mut outer_map: Vec<Option<usize>> = vec![None; contours.len()];
    for i in 0..contours.len() {
        let is_outer = depth[i] % 2 == 0;
        if is_outer {
            outer_map[i] = Some(groups.len());
            groups.push(ContourGroup { outer: contours[i].clone(), holes: Vec::new() });
        }
    }

    for i in 0..contours.len() {
        if depth[i] % 2 == 0 {
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

    GroupContoursResult::Groups(groups)
}

// Trivial grouping: pick the largest area contour as outer and treat the rest as holes.
fn group_contours_trivial(contours: &[Vec<(f32, f32)>]) -> Vec<ContourGroup> {
    if contours.is_empty() {
        return Vec::new();
    }
    let mut best_idx = 0usize;
    let mut best_area = 0.0f32;
    for (i, c) in contours.iter().enumerate() {
        let area = polygon_area_abs(c);
        if area > best_area {
            best_area = area;
            best_idx = i;
        }
    }
    let mut holes = Vec::new();
    for (i, c) in contours.iter().enumerate() {
        if i != best_idx {
            holes.push(c.clone());
        }
    }
    vec![ContourGroup { outer: contours[best_idx].clone(), holes }]
}

fn orient_group_winding(group: &mut ContourGroup) {
    let outer_ccw = polygon_area_signed(&group.outer) > 0.0;
    if !outer_ccw {
        group.outer.reverse();
    }
    let desired_hole_ccw = !outer_ccw;
    for hole in &mut group.holes {
        let hole_ccw = polygon_area_signed(hole) > 0.0;
        if hole_ccw != desired_hole_ccw {
            hole.reverse();
        }
    }
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

fn append_contour(coords: &mut Vec<f64>, out_verts: &mut Vec<Vertex2>, contour: &[Point]) {
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

fn append_contour_vertices(out_verts: &mut Vec<Vertex2>, contour: &[Point]) -> usize {
    let mut n = contour.len();
    if n >= 2 {
        let first = contour[0];
        let last = contour[n - 1];
        if (first.0 - last.0).abs() < 0.01 && (first.1 - last.1).abs() < 0.01 {
            n -= 1;
        }
    }
    for &(x, y) in contour.iter().take(n) {
        out_verts.push(Vertex2 { x: x.round() as i32, y: y.round() as i32 });
    }
    n
}

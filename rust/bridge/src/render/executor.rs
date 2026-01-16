use crate::render::device::RenderDevice;
#[cfg(debug_assertions)]
use crate::render::device::fb3ds;
use crate::render::frame::{ColorTransform, FramePacket, Matrix2D, RectI, RenderCmd, TexVertex};
use crate::render::SharedCaches;
use crate::render::cache::shapes::Vertex2;
use crate::runlog;
use crate::util::config;

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub struct CommandExecutor;

const DEBUG_AFFINE_VERTS: [Vertex2; 4] = [
    Vertex2 { x: 0, y: 0 },
    Vertex2 { x: 40, y: 0 },
    Vertex2 { x: 40, y: 20 },
    Vertex2 { x: 0, y: 20 },
];
const DEBUG_AFFINE_INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

static MESH_WARN_COUNT: AtomicU32 = AtomicU32::new(0);
static TEXTURE_WARN_COUNT: AtomicU32 = AtomicU32::new(0);
static STROKE_WARN_COUNT: AtomicU32 = AtomicU32::new(0);
static TEXT_MESH_WARN_COUNT: AtomicU32 = AtomicU32::new(0);
static MASK_WARN_COUNT: AtomicU32 = AtomicU32::new(0);
static FRAME_COUNTER: AtomicU32 = AtomicU32::new(0);
static FILL_DRAW_COUNT: AtomicU32 = AtomicU32::new(0);
static FILL_FALLBACK_COUNT: AtomicU32 = AtomicU32::new(0);
static TEXT_DRAW_COUNT: AtomicU32 = AtomicU32::new(0);
static TEXT_FALLBACK_COUNT: AtomicU32 = AtomicU32::new(0);
static STROKE_DRAW_COUNT: AtomicU32 = AtomicU32::new(0);
static STROKE_FALLBACK_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_MESH_TRIS: AtomicU32 = AtomicU32::new(0);
static LAST_RECT_FASTPATH: AtomicU32 = AtomicU32::new(0);
static LAST_BOUNDS_FALLBACKS: AtomicU32 = AtomicU32::new(0);
static FILL_ALPHA_WARN_COUNT: AtomicU32 = AtomicU32::new(0);
const DRAW_SUMMARY_FRAMES: u32 = 1800;

fn apply_color_transform_rgba(mut rgba: [u8; 4], ct: Option<ColorTransform>) -> [u8; 4] {
    if let Some(ct) = ct {
        for i in 0..4 {
            let v = rgba[i] as f32 * ct.mul[i] + ct.add[i];
            rgba[i] = v.clamp(0.0, 255.0) as u8;
        }
    }
    rgba
}

fn rect_intersects_surface(rect: RectI, sw: i32, sh: i32) -> bool {
    if rect.w <= 0 || rect.h <= 0 {
        return false;
    }
    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x + rect.w;
    let y1 = rect.y + rect.h;
    !(x1 <= 0 || y1 <= 0 || x0 >= sw || y0 >= sh)
}

fn rect_aabb_transformed(rect: RectI, transform: Matrix2D) -> RectI {
    let x0 = rect.x as f32;
    let y0 = rect.y as f32;
    let x1 = (rect.x + rect.w) as f32;
    let y1 = (rect.y + rect.h) as f32;

    let (tx0, ty0) = transform.apply(x0, y0);
    let (tx1, ty1) = transform.apply(x1, y0);
    let (tx2, ty2) = transform.apply(x1, y1);
    let (tx3, ty3) = transform.apply(x0, y1);

    let minx = tx0.min(tx1.min(tx2.min(tx3)));
    let maxx = tx0.max(tx1.max(tx2.max(tx3)));
    let miny = ty0.min(ty1.min(ty2.min(ty3)));
    let maxy = ty0.max(ty1.max(ty2.max(ty3)));

    let x = minx.floor() as i32;
    let y = miny.floor() as i32;
    let w = (maxx.ceil() as i32).saturating_sub(x);
    let h = (maxy.ceil() as i32).saturating_sub(y);
    RectI { x, y, w, h }
}

fn is_integer_translation(transform: Matrix2D) -> Option<(i32, i32)> {
    if !transform.is_translation() {
        return None;
    }
    let tx = transform.tx.round();
    let ty = transform.ty.round();
    if (transform.tx - tx).abs() <= 0.0001 && (transform.ty - ty).abs() <= 0.0001 {
        return Some((tx as i32, ty as i32));
    }
    None
}

fn mesh_is_axis_aligned_rect(mesh_verts: &[crate::render::cache::shapes::Vertex2], indices: &[u16]) -> Option<RectI> {
    // Fast-path: the common 2-triangle rectangle mesh.
    if mesh_verts.len() != 4 || indices.len() != 6 {
        return None;
    }
    if indices != [0, 1, 2, 0, 2, 3] {
        return None;
    }

    let v0 = mesh_verts[0];
    let v1 = mesh_verts[1];
    let v2 = mesh_verts[2];
    let v3 = mesh_verts[3];

    // Expect (x0,y0) (x1,y0) (x1,y1) (x0,y1)
    if v0.y != v1.y || v2.y != v3.y || v0.x != v3.x || v1.x != v2.x {
        return None;
    }
    let x0 = v0.x;
    let y0 = v0.y;
    let x1 = v1.x;
    let y1 = v2.y;
    let w = x1 - x0;
    let h = y1 - y0;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(RectI { x: x0, y: y0, w, h })
}

impl CommandExecutor {
    pub fn new() -> Self { Self }

    pub fn execute<D: RenderDevice>(&mut self, packet: &FramePacket, device: &mut D, caches: &SharedCaches) {
        let sw = device.surface_width();
        let sh = device.surface_height();

        // Lock caches once per frame.
        let bitmaps = caches.bitmaps.lock().unwrap();
        let shapes = caches.shapes.lock().unwrap();
        let mut mask_stack: Vec<RectI> = Vec::new();

        let mut mesh_tris = 0u32;
        let mut rect_fastpath = 0u32;
        let mut bounds_fallbacks = 0u32;

        for cmd in &packet.cmds {
            match cmd {
                RenderCmd::FillRect { rect, color_key, wireframe } => {
                    let (cr, cg, cb) = color_from_key(*color_key);
                    device.fill_rect(*rect, cr, cg, cb);
                    if *wireframe {
                        device.stroke_rect(*rect, 255, 255, 255);
                    }
                }
                RenderCmd::DrawShapeSolidFill { shape_key, fill_idx, transform, solid_rgba, color_transform, color_key, wireframe } => {
                    FILL_DRAW_COUNT.fetch_add(1, Ordering::Relaxed);
                    let solid_rgba = solid_rgba.map(|rgba| apply_color_transform_rgba(rgba, *color_transform));
                    let (fallback_r, fallback_g, fallback_b) = if let Some([r, g, b, a]) = solid_rgba {
                        if a != 255 && FILL_ALPHA_WARN_COUNT.fetch_add(1, Ordering::Relaxed) < 4 {
                            // Alpha blending for vector fills is future work; current Step 3 is opaque.
                            runlog::warn_line("fill_alpha ignored; vector fills are opaque in Step 3");
                        }
                        (r, g, b)
                    } else {
                        color_from_key(*color_key)
                    };
                    // Early reject by transformed bounds (very common win for offscreen sprites).
                    if let Some(b) = shapes.get_bounds(*shape_key) {
                        let tr = rect_aabb_transformed(b, *transform);
                        if !rect_intersects_surface(tr, sw, sh) {
                            continue;
                        }
                    }

                    let int_translation = is_integer_translation(*transform);
                    let mut used_fallback = false;
                    let mut missing_mesh = false;
                    let mut invalid_mesh = false;
                    if let Some(mesh) = shapes.get_fill_mesh(*shape_key, *fill_idx as usize) {
                        let (cr, cg, cb) = (fallback_r, fallback_g, fallback_b);
                        let indices_ok = !mesh.indices.is_empty() && mesh.indices.len() % 3 == 0;
                        let verts_ok = !mesh.verts.is_empty();
                        if indices_ok && verts_ok {
                            // Fast-path: common rect mesh is much faster to draw with `fill_rect`.
                            if let Some(local) = mesh_is_axis_aligned_rect(&mesh.verts, &mesh.indices) {
                                if let Some((tx, ty)) = int_translation {
                                    rect_fastpath = rect_fastpath.saturating_add(1);
                                    let rect = RectI { x: local.x + tx, y: local.y + ty, w: local.w, h: local.h };
                                    device.fill_rect(rect, cr, cg, cb);
                                    if *wireframe {
                                        device.stroke_rect(rect, 255, 255, 255);
                                    }
                                } else if transform.is_axis_aligned() {
                                    rect_fastpath = rect_fastpath.saturating_add(1);
                                    let x0 = local.x as f32;
                                    let y0 = local.y as f32;
                                    let x1 = (local.x + local.w) as f32;
                                    let y1 = (local.y + local.h) as f32;
                                    let tx0 = transform.a * x0 + transform.tx;
                                    let tx1 = transform.a * x1 + transform.tx;
                                    let ty0 = transform.d * y0 + transform.ty;
                                    let ty1 = transform.d * y1 + transform.ty;
                                    let minx = tx0.min(tx1);
                                    let maxx = tx0.max(tx1);
                                    let miny = ty0.min(ty1);
                                    let maxy = ty0.max(ty1);
                                    let x = minx.floor() as i32;
                                    let y = miny.floor() as i32;
                                    let w = (maxx.ceil() as i32).saturating_sub(x);
                                    let h = (maxy.ceil() as i32).saturating_sub(y);
                                    if w > 0 && h > 0 {
                                        let rect = RectI { x, y, w, h };
                                        device.fill_rect(rect, cr, cg, cb);
                                        if *wireframe {
                                            device.stroke_rect(rect, 255, 255, 255);
                                        }
                                    }
                                } else {
                                    mesh_tris = mesh_tris.saturating_add((mesh.indices.len() as u32) / 3);
                                    device.fill_tris_solid_affine(&mesh.verts, &mesh.indices, *transform, cr, cg, cb);
                                    if *wireframe {
                                        device.draw_tris_wireframe_affine(&mesh.verts, &mesh.indices, *transform, 255, 255, 255);
                                    }
                                }
                            } else if let Some((tx, ty)) = int_translation {
                                mesh_tris = mesh_tris.saturating_add((mesh.indices.len() as u32) / 3);
                                device.fill_tris_solid(&mesh.verts, &mesh.indices, tx, ty, cr, cg, cb);
                                if *wireframe {
                                    device.draw_tris_wireframe(&mesh.verts, &mesh.indices, tx, ty, 255, 255, 255);
                                }
                            } else {
                                mesh_tris = mesh_tris.saturating_add((mesh.indices.len() as u32) / 3);
                                device.fill_tris_solid_affine(&mesh.verts, &mesh.indices, *transform, cr, cg, cb);
                                if *wireframe {
                                    device.draw_tris_wireframe_affine(&mesh.verts, &mesh.indices, *transform, 255, 255, 255);
                                }
                            }
                        } else {
                            shapes.record_invalid_fill_mesh();
                            invalid_mesh = true;
                            used_fallback = true;
                        }
                    } else {
                        shapes.record_missing_fill_mesh();
                        missing_mesh = true;
                        used_fallback = true;
                    }

                    if used_fallback {
                        FILL_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                        if missing_mesh || invalid_mesh {
                            let n = MESH_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                            if n < 8 {
                                let kind = if missing_mesh { "missing_mesh" } else { "invalid_mesh" };
                                runlog::warn_line(&format!(
                                    "fill_fallback {} shape={} fill={}",
                                    kind, shape_key, fill_idx
                                ));
                            }
                        }
                        if let Some(b) = shapes.get_bounds(*shape_key) {
                            shapes.record_bounds_fallback();
                            bounds_fallbacks = bounds_fallbacks.saturating_add(1);
                            // Safe fallback: bounds rect.
                            let rect = rect_aabb_transformed(b, *transform);
                            device.fill_rect(rect, fallback_r, fallback_g, fallback_b);
                            if *wireframe {
                                device.stroke_rect(rect, 255, 255, 255);
                            }
                        }
                    }
                }
                RenderCmd::DrawTextSolidFill { shape_key, fill_idx, transform, solid_rgba, color_transform, color_key, wireframe } => {
                    TEXT_DRAW_COUNT.fetch_add(1, Ordering::Relaxed);
                    let solid_rgba = solid_rgba.map(|rgba| apply_color_transform_rgba(rgba, *color_transform));
                    let (fallback_r, fallback_g, fallback_b) = if let Some([r, g, b, a]) = solid_rgba {
                        if a != 255 && FILL_ALPHA_WARN_COUNT.fetch_add(1, Ordering::Relaxed) < 4 {
                            // Alpha blending for vector fills is future work; current Step 3 is opaque.
                            runlog::warn_line("fill_alpha ignored; vector fills are opaque in Step 3");
                        }
                        (r, g, b)
                    } else {
                        color_from_key(*color_key)
                    };
                    // Early reject by transformed bounds (very common win for offscreen text).
                    if let Some(b) = shapes.get_bounds(*shape_key) {
                        let tr = rect_aabb_transformed(b, *transform);
                        if !rect_intersects_surface(tr, sw, sh) {
                            continue;
                        }
                    }
                    let int_translation = is_integer_translation(*transform);
                    let mut used_fallback = false;
                    let mut missing_mesh = false;
                    let mut invalid_mesh = false;
                    if let Some(mesh) = shapes.get_fill_mesh(*shape_key, *fill_idx as usize) {
                        let (cr, cg, cb) = (fallback_r, fallback_g, fallback_b);
                        let indices_ok = !mesh.indices.is_empty() && mesh.indices.len() % 3 == 0;
                        let verts_ok = !mesh.verts.is_empty();
                        if indices_ok && verts_ok {
                            mesh_tris = mesh_tris.saturating_add((mesh.indices.len() as u32) / 3);
                            if let Some((tx, ty)) = int_translation {
                                device.fill_tris_solid(&mesh.verts, &mesh.indices, tx, ty, cr, cg, cb);
                                if *wireframe {
                                    device.draw_tris_wireframe(&mesh.verts, &mesh.indices, tx, ty, 255, 255, 255);
                                }
                            } else {
                                device.fill_tris_solid_affine(&mesh.verts, &mesh.indices, *transform, cr, cg, cb);
                                if *wireframe {
                                    device.draw_tris_wireframe_affine(&mesh.verts, &mesh.indices, *transform, 255, 255, 255);
                                }
                            }
                        } else {
                            shapes.record_invalid_fill_mesh();
                            invalid_mesh = true;
                            used_fallback = true;
                        }
                    } else {
                        shapes.record_missing_fill_mesh();
                        missing_mesh = true;
                        used_fallback = true;
                    }

                    if used_fallback {
                        TEXT_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                        let n = TEXT_MESH_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                        if n < 8 {
                            let kind = if missing_mesh { "missing_mesh" } else { "invalid_mesh" };
                            runlog::warn_line(&format!(
                                "text_fill_fallback {} shape={} fill={}",
                                kind, shape_key, fill_idx
                            ));
                        }
                        if let Some(b) = shapes.get_bounds(*shape_key) {
                            bounds_fallbacks = bounds_fallbacks.saturating_add(1);
                            let rect = rect_aabb_transformed(b, *transform);
                            device.fill_rect(rect, fallback_r, fallback_g, fallback_b);
                            if *wireframe {
                                device.stroke_rect(rect, 255, 255, 255);
                            }
                        }
                    }
                }
                RenderCmd::DrawShapeStroke { shape_key, stroke_idx, transform, r, g, b, wireframe } => {
                    STROKE_DRAW_COUNT.fetch_add(1, Ordering::Relaxed);
                    // Early reject by transformed bounds (very common win for offscreen strokes).
                    if let Some(b) = shapes.get_bounds(*shape_key) {
                        let tr = rect_aabb_transformed(b, *transform);
                        if !rect_intersects_surface(tr, sw, sh) {
                            continue;
                        }
                    }
                    let int_translation = is_integer_translation(*transform);
                    let mut used_fallback = false;
                    let mut missing_mesh = false;
                    let mut invalid_mesh = false;
                    if let Some(mesh) = shapes.get_stroke_mesh(*shape_key, *stroke_idx as usize) {
                        let indices_ok = !mesh.indices.is_empty() && mesh.indices.len() % 3 == 0;
                        let verts_ok = !mesh.verts.is_empty();
                        if indices_ok && verts_ok {
                            mesh_tris = mesh_tris.saturating_add((mesh.indices.len() as u32) / 3);
                            if let Some((tx, ty)) = int_translation {
                                device.fill_tris_solid(&mesh.verts, &mesh.indices, tx, ty, *r, *g, *b);
                                if *wireframe {
                                    device.draw_tris_wireframe(&mesh.verts, &mesh.indices, tx, ty, 255, 255, 255);
                                }
                            } else {
                                device.fill_tris_solid_affine(&mesh.verts, &mesh.indices, *transform, *r, *g, *b);
                                if *wireframe {
                                    device.draw_tris_wireframe_affine(&mesh.verts, &mesh.indices, *transform, 255, 255, 255);
                                }
                            }
                        } else {
                            shapes.record_invalid_stroke_mesh();
                            invalid_mesh = true;
                            used_fallback = true;
                        }
                    } else {
                        shapes.record_missing_stroke_mesh();
                        missing_mesh = true;
                        used_fallback = true;
                    }

                    if used_fallback {
                        STROKE_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
                        let n = STROKE_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                        if n < 8 {
                            let kind = if missing_mesh { "missing_mesh" } else { "invalid_mesh" };
                            runlog::warn_line(&format!(
                                "stroke_fallback {} shape={} stroke={}",
                                kind, shape_key, stroke_idx
                            ));
                        }
                        if let Some(bnd) = shapes.get_bounds(*shape_key) {
                            shapes.record_stroke_bounds_fallback();
                            bounds_fallbacks = bounds_fallbacks.saturating_add(1);
                            let rect = rect_aabb_transformed(bnd, *transform);
                            device.stroke_rect(rect, *r, *g, *b);
                        }
                    }
                }
                RenderCmd::PushMaskRect { rect } => {
                    if !config::masks_enabled() {
                        let n = MASK_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                        if n < 4 {
                            runlog::warn_line("masks disabled; ignoring mask");
                        }
                        continue;
                    }
                    let mut next = *rect;
                    if let Some(prev) = mask_stack.last() {
                        let x0 = next.x.max(prev.x);
                        let y0 = next.y.max(prev.y);
                        let x1 = (next.x + next.w).min(prev.x + prev.w);
                        let y1 = (next.y + next.h).min(prev.y + prev.h);
                        next = RectI { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) };
                    }
                    mask_stack.push(next);
                    device.set_scissor(Some(next));
                }
                RenderCmd::PushMaskShape { .. } => {
                    let n = MASK_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                    if n < 4 {
                        runlog::warn_line("shape masks unsupported; ignoring");
                    }
                }
                RenderCmd::PopMask => {
                    if mask_stack.pop().is_some() {
                        let rect = mask_stack.last().copied();
                        device.set_scissor(rect);
                    } else {
                        let n = MASK_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                        if n < 4 {
                            runlog::warn_line("mask stack underflow");
                        }
                        device.set_scissor(None);
                    }
                }
                RenderCmd::BlitBitmap { bitmap_key, transform, uv, color_transform } => {
                    if let Some(src) = bitmaps.get(*bitmap_key) {
                        let use_blit = transform.is_identity() && uv.is_full() && color_transform.is_none();
                        if use_blit {
                            device.blit_rgba(transform.tx.round() as i32, transform.ty.round() as i32, src);
                            continue;
                        }

                        if !config::textured_bitmaps_enabled() {
                            let n = TEXTURE_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                            if n < 4 {
                                runlog::warn_line("textured_bitmaps disabled; skipping transformed bitmap");
                            }
                            continue;
                        }

                        let w = src.width as f32;
                        let h = src.height as f32;
                        let (x0, y0) = transform.apply(0.0, 0.0);
                        let (x1, y1) = transform.apply(w, 0.0);
                        let (x2, y2) = transform.apply(w, h);
                        let (x3, y3) = transform.apply(0.0, h);

                        let verts = [
                            TexVertex { x: x0, y: y0, u: uv.u0, v: uv.v0 },
                            TexVertex { x: x1, y: y1, u: uv.u1, v: uv.v0 },
                            TexVertex { x: x2, y: y2, u: uv.u1, v: uv.v1 },
                            TexVertex { x: x3, y: y3, u: uv.u0, v: uv.v1 },
                        ];
                        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
                        device.draw_tris_textured(&verts, &indices, src, *color_transform);
                    }
                }
                RenderCmd::DebugAffineRect { transform, r, g, b } => {
                    device.fill_tris_solid_affine(&DEBUG_AFFINE_VERTS, &DEBUG_AFFINE_INDICES, *transform, *r, *g, *b);
                    device.draw_tris_wireframe_affine(&DEBUG_AFFINE_VERTS, &DEBUG_AFFINE_INDICES, *transform, 255, 255, 255);
                }
                RenderCmd::DebugLoadingIndicator => {
                    // More intuitive "loading" indicator without text:
                    // a bordered bar with an animated highlight moving left→right.
                    //
                    // NOTE: We intentionally keep this inside the executor so it stays
                    // device-agnostic and doesn't require a time source from the platform.
                    static TICK: AtomicU32 = AtomicU32::new(0);
                    let t = TICK.fetch_add(1, Ordering::Relaxed);

                    // Bar geometry (centered for 400x240 top screen).
                    let x0 = 90;
                    let y0 = 108;
                    let w = 220;
                    let h = 24;

                    // Background + border
                    device.fill_rect(RectI { x: x0, y: y0, w, h }, 30, 30, 30);
                    // Top border
                    device.fill_rect(RectI { x: x0, y: y0, w, h: 2 }, 120, 120, 120);
                    // Bottom border
                    device.fill_rect(RectI { x: x0, y: y0 + h - 2, w, h: 2 }, 120, 120, 120);
                    // Left border
                    device.fill_rect(RectI { x: x0, y: y0, w: 2, h }, 120, 120, 120);
                    // Right border
                    device.fill_rect(RectI { x: x0 + w - 2, y: y0, w: 2, h }, 120, 120, 120);

                    // Animated highlight segment inside the bar.
                    let inner_x = x0 + 4;
                    let inner_y = y0 + 4;
                    let inner_w = w - 8;
                    let inner_h = h - 8;
                    let seg_w = 44;
                    let max_x = (inner_w - seg_w).max(1);
                    let seg_x = inner_x + ((t % (max_x as u32 + 1)) as i32);
                    device.fill_rect(RectI { x: seg_x, y: inner_y, w: seg_w, h: inner_h }, 200, 200, 200);

                    // "Ellipsis" dots under the bar to make it obvious it's a waiting state.
                    let dots_y = y0 + h + 10;
                    let dots_x = 182;
                    let phase = (t / 12) % 4; // 0..3
                    for i in 0..3 {
                        let on = (i as u32) < phase;
                        let c = if on { 200 } else { 60 };
                        device.fill_rect(RectI { x: dots_x + (i * 12), y: dots_y, w: 6, h: 6 }, c, c, c);
                    }
                }
            }
        }

        LAST_MESH_TRIS.store(mesh_tris, Ordering::Relaxed);
        LAST_RECT_FASTPATH.store(rect_fastpath, Ordering::Relaxed);
        LAST_BOUNDS_FALLBACKS.store(bounds_fallbacks, Ordering::Relaxed);

        let frame = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if runlog::is_verbose() && frame % DRAW_SUMMARY_FRAMES == 0 {
            let fill_draws = FILL_DRAW_COUNT.swap(0, Ordering::Relaxed);
            let fill_fallbacks = FILL_FALLBACK_COUNT.swap(0, Ordering::Relaxed);
            let text_draws = TEXT_DRAW_COUNT.swap(0, Ordering::Relaxed);
            let text_fallbacks = TEXT_FALLBACK_COUNT.swap(0, Ordering::Relaxed);
            let stroke_draws = STROKE_DRAW_COUNT.swap(0, Ordering::Relaxed);
            let stroke_fallbacks = STROKE_FALLBACK_COUNT.swap(0, Ordering::Relaxed);
            runlog::log_line(&format!(
                "draw_summary frames={} fill_fallbacks={}/{} text_fallbacks={}/{} stroke_fallbacks={}/{}",
                frame,
                fill_fallbacks,
                fill_draws,
                text_fallbacks,
                text_draws,
                stroke_fallbacks,
                stroke_draws
            ));
            #[cfg(debug_assertions)]
            {
                let (translation, axis_aligned, general) = fb3ds::take_affine_path_counts();
                runlog::log_line(&format!(
                    "affine_paths translation={} axis_aligned={} general={}",
                    translation,
                    axis_aligned,
                    general
                ));
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DrawStats {
    pub mesh_tris: u32,
    pub rect_fastpath: u32,
    pub bounds_fallbacks: u32,
}

pub fn last_draw_stats() -> DrawStats {
    DrawStats {
        mesh_tris: LAST_MESH_TRIS.load(Ordering::Relaxed),
        rect_fastpath: LAST_RECT_FASTPATH.load(Ordering::Relaxed),
        bounds_fallbacks: LAST_BOUNDS_FALLBACKS.load(Ordering::Relaxed),
    }
}

fn color_from_key(mut k: u64) -> (u8, u8, u8) {
    // Deterministic hash -> visible colors.
    k = k.wrapping_mul(0x9E3779B185EBCA87);
    k ^= k >> 33;
    k = k.wrapping_mul(0xC2B2AE3D27D4EB4F);
    k ^= k >> 29;
    let r = (k & 0xFF) as u8;
    let g = ((k >> 8) & 0xFF) as u8;
    let b = ((k >> 16) & 0xFF) as u8;
    (r, g, b)
}

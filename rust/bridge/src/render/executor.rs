use crate::render::device::RenderDevice;
use crate::render::frame::{FramePacket, RectI, RenderCmd};
use crate::render::SharedCaches;

use core::sync::atomic::{AtomicU32, Ordering};

pub struct CommandExecutor;

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

        for cmd in &packet.cmds {
            match cmd {
                RenderCmd::FillRect { rect, color_key, wireframe } => {
                    let (cr, cg, cb) = color_from_key(*color_key);
                    device.fill_rect(*rect, cr, cg, cb);
                    if *wireframe {
                        device.stroke_rect(*rect, 255, 255, 255);
                    }
                }
                RenderCmd::DrawShapeSolidFill { shape_key, fill_idx, tx, ty, color_key, wireframe } => {
                    let (cr, cg, cb) = color_from_key(*color_key);
                    // Early reject by translated bounds (very common win for offscreen sprites).
                    if let Some(b) = shapes.get_bounds(*shape_key) {
                        let tr = RectI { x: b.x + *tx, y: b.y + *ty, w: b.w, h: b.h };
                        if !rect_intersects_surface(tr, sw, sh) {
                            continue;
                        }
                    }

                    if let Some(mesh) = shapes.get_fill_mesh(*shape_key, *fill_idx as usize) {
                        // Fast-path: common rect mesh is much faster to draw with `fill_rect`.
                        if let Some(local) = mesh_is_axis_aligned_rect(&mesh.verts, &mesh.indices) {
                            let rect = RectI { x: local.x + *tx, y: local.y + *ty, w: local.w, h: local.h };
                            device.fill_rect(rect, cr, cg, cb);
                            if *wireframe {
                                device.stroke_rect(rect, 255, 255, 255);
                            }
                        } else {
                            device.fill_tris_solid(&mesh.verts, &mesh.indices, *tx, *ty, cr, cg, cb);
                            if *wireframe {
                                device.draw_tris_wireframe(&mesh.verts, &mesh.indices, *tx, *ty, 255, 255, 255);
                            }
                        }
                    } else if let Some(b) = shapes.get_bounds(*shape_key) {
                        // Safe fallback: bounds rect.
                        let rect = RectI { x: b.x + *tx, y: b.y + *ty, w: b.w, h: b.h };
                        device.fill_rect(rect, cr, cg, cb);
                        if *wireframe {
                            device.stroke_rect(rect, 255, 255, 255);
                        }
                    }
                }
                RenderCmd::BlitBitmap { x, y, bitmap_key } => {
                    if let Some(src) = bitmaps.get(*bitmap_key) {
                        device.blit_rgba(*x, *y, src);
                    }
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

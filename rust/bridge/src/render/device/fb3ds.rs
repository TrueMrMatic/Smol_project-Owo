use crate::render::device::RenderDevice;
use crate::render::frame::{ClearColor, ColorTransform, RectI, TexVertex};
use crate::render::cache::bitmaps::BitmapSurface;
use crate::render::cache::shapes::Vertex2;

extern "C" {
    fn gfxGetFramebuffer(screen: i32, side: i32, width: *mut u16, height: *mut u16) -> *mut u8;
}

const GFX_TOP: i32 = 0;
const GFX_LEFT: i32 = 0;

#[derive(Clone, Copy)]
struct FbView {
    ptr: *mut u8,
    w_mem: usize,
    h_mem: usize,
    scissor: Option<RectI>,
}

impl FbView {
    #[inline(always)]
    fn disp_w(&self) -> usize { self.h_mem }
    #[inline(always)]
    fn disp_h(&self) -> usize { self.w_mem }

    #[inline(always)]
    unsafe fn put_pixel(&self, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if x < 0 || y < 0 { return; }
        if let Some(scissor) = self.scissor {
            if x < scissor.x || y < scissor.y || x >= scissor.x + scissor.w || y >= scissor.y + scissor.h {
                return;
            }
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.disp_w() || y >= self.disp_h() { return; }
        let idx = 3 * (x * self.w_mem + (self.w_mem - 1 - y));
        let p = self.ptr.add(idx);
        *p.add(0) = b;
        *p.add(1) = g;
        *p.add(2) = r;
    }

    unsafe fn clear(&self, r: u8, g: u8, b: u8) {
        let count = self.w_mem * self.h_mem;
        let mut p = self.ptr;
        for _ in 0..count {
            *p.add(0) = b;
            *p.add(1) = g;
            *p.add(2) = r;
            p = p.add(3);
        }
    }

    unsafe fn fill_rect(&self, x0: i32, y0: i32, w: i32, h: i32, r: u8, g: u8, b: u8) {
        if w <= 0 || h <= 0 { return; }
        let x1 = x0 + w;
        let y1 = y0 + h;

        // Clip to display coords.
        let cx0 = x0.max(0);
        let cy0 = y0.max(0);
        let cx1 = x1.min(self.disp_w() as i32);
        let cy1 = y1.min(self.disp_h() as i32);

        let (mut cx0, mut cy0, mut cx1, mut cy1) = (cx0, cy0, cx1, cy1);
        if let Some(scissor) = self.scissor {
            cx0 = cx0.max(scissor.x);
            cy0 = cy0.max(scissor.y);
            cx1 = cx1.min(scissor.x + scissor.w);
            cy1 = cy1.min(scissor.y + scissor.h);
        }
        if cx1 <= cx0 || cy1 <= cy0 { return; }

        // IMPORTANT (3DS framebuffer layout):
        // The top framebuffer is stored rotated. Our put_pixel mapping is:
        //   idx = 3 * (x * w_mem + (w_mem - 1 - y))
        // So for a fixed x, varying y is contiguous in memory (reverse order).
        // Looping x outer + y inner is significantly faster than y outer + x inner.

        let w_mem_i32 = self.w_mem as i32;
        let row_stride = self.w_mem; // pixels

        // Iterate each display-x (memory row) and fill a contiguous span of columns.
        for x in cx0..cx1 {
            // Start at y = cy1-1 so we can increment forward in memory.
            let start_col = (w_mem_i32 - cy1) as usize; // col = w_mem - 1 - (cy1-1)
            let base = 3 * ((x as usize) * row_stride + start_col);
            let mut p = self.ptr.add(base);
            for _y in (cy0..cy1).rev() {
                *p.add(0) = b;
                *p.add(1) = g;
                *p.add(2) = r;
                p = p.add(3);
            }
        }
    }

    unsafe fn blit_rgba(&self, dst_x0: i32, dst_y0: i32, src: &BitmapSurface) {
        let src_w = src.width as i32;
        let src_h = src.height as i32;
        if src_w <= 0 || src_h <= 0 { return; }

        // Clip dest rect to display coords.
        let x0 = dst_x0;
        let y0 = dst_y0;
        let x1 = dst_x0 + src_w;
        let y1 = dst_y0 + src_h;

        let cx0 = x0.max(0);
        let cy0 = y0.max(0);
        let cx1 = x1.min(self.disp_w() as i32);
        let cy1 = y1.min(self.disp_h() as i32);
        let (mut cx0, mut cy0, mut cx1, mut cy1) = (cx0, cy0, cx1, cy1);
        if let Some(scissor) = self.scissor {
            cx0 = cx0.max(scissor.x);
            cy0 = cy0.max(scissor.y);
            cx1 = cx1.min(scissor.x + scissor.w);
            cy1 = cy1.min(scissor.y + scissor.h);
        }
        if cx1 <= cx0 || cy1 <= cy0 { return; }

        let w_mem_i32 = self.w_mem as i32;
        let row_stride = self.w_mem; // pixels

        // Iterate display-x outer, display-y inner (reverse), to keep framebuffer writes contiguous.
        for x in cx0..cx1 {
            let sx = x - dst_x0;
            if sx < 0 || sx >= src_w { continue; }

            let start_col = (w_mem_i32 - cy1) as usize;
            let base = 3 * ((x as usize) * row_stride + start_col);
            let mut p = self.ptr.add(base);

            for y in (cy0..cy1).rev() {
                let sy = y - dst_y0;
                if sy >= 0 && sy < src_h {
                    let si = 4 * ((sy as usize) * (src.width as usize) + (sx as usize));
                    let sr = src.rgba[si + 0];
                    let sg = src.rgba[si + 1];
                    let sb = src.rgba[si + 2];
                    let sa = src.rgba[si + 3];
                    if src.is_opaque {
                        *p.add(0) = sb;
                        *p.add(1) = sg;
                        *p.add(2) = sr;
                    } else if sa == 255 {
                        *p.add(0) = sb;
                        *p.add(1) = sg;
                        *p.add(2) = sr;
                    } else if sa != 0 {
                        // Straight-alpha blend: out = src*a + dst*(1-a)
                        let inv = 255u16 - sa as u16;
                        let db = *p.add(0) as u16;
                        let dg = *p.add(1) as u16;
                        let dr = *p.add(2) as u16;

                        let ob = ((sb as u16 * sa as u16 + db * inv + 127) / 255) as u8;
                        let og = ((sg as u16 * sa as u16 + dg * inv + 127) / 255) as u8;
                        let or = ((sr as u16 * sa as u16 + dr * inv + 127) / 255) as u8;

                        *p.add(0) = ob;
                        *p.add(1) = og;
                        *p.add(2) = or;
                    }
                }
                p = p.add(3);
            }
        }
    }

    #[inline(always)]
    fn apply_color_transform(src: [u8; 4], ct: Option<ColorTransform>) -> [u8; 4] {
        if let Some(ct) = ct {
            let mut out = [0u8; 4];
            for i in 0..4 {
                let v = src[i] as f32 * ct.mul[i] + ct.add[i];
                out[i] = v.clamp(0.0, 255.0) as u8;
            }
            out
        } else {
            src
        }
    }

    unsafe fn draw_triangle_textured(
        &self,
        v0: TexVertex,
        v1: TexVertex,
        v2: TexVertex,
        src: &BitmapSurface,
        color_transform: Option<ColorTransform>,
    ) {
        let (minx, maxx) = (v0.x.min(v1.x.min(v2.x)), v0.x.max(v1.x.max(v2.x)));
        let (miny, maxy) = (v0.y.min(v1.y.min(v2.y)), v0.y.max(v1.y.max(v2.y)));
        let mut ix0 = minx.floor() as i32;
        let mut ix1 = maxx.ceil() as i32;
        let mut iy0 = miny.floor() as i32;
        let mut iy1 = maxy.ceil() as i32;

        let disp_w = self.disp_w() as i32;
        let disp_h = self.disp_h() as i32;
        if ix1 < 0 || iy1 < 0 || ix0 >= disp_w || iy0 >= disp_h {
            return;
        }
        ix0 = ix0.max(0);
        iy0 = iy0.max(0);
        ix1 = ix1.min(disp_w - 1);
        iy1 = iy1.min(disp_h - 1);
        if let Some(scissor) = self.scissor {
            ix0 = ix0.max(scissor.x);
            iy0 = iy0.max(scissor.y);
            ix1 = ix1.min(scissor.x + scissor.w - 1);
            iy1 = iy1.min(scissor.y + scissor.h - 1);
        }

        let area = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
        if area.abs() <= f32::EPSILON {
            return;
        }
        let inv_area = 1.0 / area;

        let w_mem_i32 = self.w_mem as i32;
        let row_stride = self.w_mem;

        for x in ix0..=ix1 {
            let start_col = (w_mem_i32 - (iy1 + 1)) as usize;
            let base = 3 * ((x as usize) * row_stride + start_col);
            let mut p = self.ptr.add(base);
            for y in (iy0..=iy1).rev() {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;

                let w0 = (v1.x - v0.x) * (py - v0.y) - (v1.y - v0.y) * (px - v0.x);
                let w1 = (v2.x - v1.x) * (py - v1.y) - (v2.y - v1.y) * (px - v1.x);
                let w2 = (v0.x - v2.x) * (py - v2.y) - (v0.y - v2.y) * (px - v2.x);

                if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                    let l0 = ((v1.x - px) * (v2.y - py) - (v1.y - py) * (v2.x - px)) * inv_area;
                    let l1 = ((v2.x - px) * (v0.y - py) - (v2.y - py) * (v0.x - px)) * inv_area;
                    let l2 = 1.0 - l0 - l1;

                    let u = v0.u * l0 + v1.u * l1 + v2.u * l2;
                    let v = v0.v * l0 + v1.v * l1 + v2.v * l2;
                    let sx = (u.clamp(0.0, 1.0) * (src.width as f32 - 1.0)).round() as i32;
                    let sy = (v.clamp(0.0, 1.0) * (src.height as f32 - 1.0)).round() as i32;

                    if sx >= 0 && sy >= 0 && sx < src.width as i32 && sy < src.height as i32 {
                        let si = 4 * ((sy as usize) * (src.width as usize) + (sx as usize));
                        let tex = [
                            src.rgba[si + 0],
                            src.rgba[si + 1],
                            src.rgba[si + 2],
                            src.rgba[si + 3],
                        ];
                        let tex = FbView::apply_color_transform(tex, color_transform);
                        let sr = tex[0];
                        let sg = tex[1];
                        let sb = tex[2];
                        let sa = tex[3];

                        if src.is_opaque && color_transform.is_none() {
                            *p.add(0) = sb;
                            *p.add(1) = sg;
                            *p.add(2) = sr;
                        } else if sa == 255 {
                            *p.add(0) = sb;
                            *p.add(1) = sg;
                            *p.add(2) = sr;
                        } else if sa != 0 {
                            let inv = 255u16 - sa as u16;
                            let db = *p.add(0) as u16;
                            let dg = *p.add(1) as u16;
                            let dr = *p.add(2) as u16;

                            let ob = ((sb as u16 * sa as u16 + db * inv + 127) / 255) as u8;
                            let og = ((sg as u16 * sa as u16 + dg * inv + 127) / 255) as u8;
                            let or = ((sr as u16 * sa as u16 + dr * inv + 127) / 255) as u8;

                            *p.add(0) = ob;
                            *p.add(1) = og;
                            *p.add(2) = or;
                        }
                    }
                }
                p = p.add(3);
            }
        }
    }
}


// -----------------------------
// Triangle rasterization (opaque solid) + optional wireframe
// -----------------------------
//
// The 3DS top framebuffer is stored rotated. Our `put_pixel` mapping means:
// for a fixed display-x, varying display-y maps to contiguous memory.
// For performance, the solid fill uses an x-major scan (vertical spans).

impl FbView {
    #[inline(always)]
    unsafe fn fill_col_span(&self, x: i32, y0: i32, y1_excl: i32, r: u8, g: u8, b: u8) {
        if x < 0 || x >= self.disp_w() as i32 { return; }
        let mut cy0 = y0.max(0);
        let mut cy1 = y1_excl.min(self.disp_h() as i32);
        if let Some(scissor) = self.scissor {
            if x < scissor.x || x >= scissor.x + scissor.w {
                return;
            }
            cy0 = cy0.max(scissor.y);
            cy1 = cy1.min(scissor.y + scissor.h);
        }
        if cy1 <= cy0 { return; }

        let w_mem_i32 = self.w_mem as i32;
        let row_stride = self.w_mem; // pixels

        // Start at y=cy1-1 so we can increment forward in memory.
        let start_col = (w_mem_i32 - cy1) as usize;
        let base = 3 * ((x as usize) * row_stride + start_col);
        let mut p = self.ptr.add(base);
        for _ in (cy0..cy1).rev() {
            *p.add(0) = b;
            *p.add(1) = g;
            *p.add(2) = r;
            p = p.add(3);
        }
    }

    #[inline(always)]
    unsafe fn fill_triangle_solid(&self, a: Vertex2, b: Vertex2, c: Vertex2, tx: i32, ty: i32, r: u8, g: u8, bcol: u8) {
        // Apply translation.
        let ax = a.x + tx; let ay = a.y + ty;
        let bx = b.x + tx; let by = b.y + ty;
        let cx = c.x + tx; let cy = c.y + ty;

        // Degenerate reject (area == 0).
        // This avoids wasting time on tiny/flat triangles produced by tessellation.
        let area2 = (bx - ax) as i64 * (cy - ay) as i64 - (by - ay) as i64 * (cx - ax) as i64;
        if area2 == 0 { return; }

        // Bounding box in X/Y (display coords) for quick reject.
        let mut minx = ax.min(bx.min(cx));
        let mut maxx = ax.max(bx.max(cx));
        let miny = ay.min(by.min(cy));
        let maxy = ay.max(by.max(cy));

        // Quick reject / clip.
        let disp_w = self.disp_w() as i32;
        let disp_h = self.disp_h() as i32;
        if maxx < 0 || minx >= disp_w { return; }
        if maxy < 0 || miny >= disp_h { return; }
        minx = minx.max(0);
        maxx = maxx.min(disp_w - 1);
        if maxx < minx { return; }

        #[derive(Clone, Copy)]
        struct Edge {
            x_start: i32,
            x_end: i32,
            y_fp: i64,
            step: i64,
        }

        let mut edges: [Option<Edge>; 3] = [None, None, None];
        let verts = [(ax, ay), (bx, by), (cx, cy)];

        for e in 0..3 {
            let (x0, y0) = verts[e];
            let (x1, y1) = verts[(e + 1) % 3];
            if x0 == x1 {
                continue;
            }
            let (sx, sy, ex, ey) = if x0 < x1 { (x0, y0, x1, y1) } else { (x1, y1, x0, y0) };
            let x_start = sx.max(minx);
            let x_end = ex.min(maxx + 1);
            if x_end <= x_start {
                continue;
            }
            let dx = (ex - sx) as i64;
            let dy = (ey - sy) as i64;
            let step = (dy << 16) / dx;
            let mut y_fp = (sy as i64) << 16;
            let advance = (x_start - sx) as i64;
            y_fp += step * advance;
            let slot = edges.iter_mut().find(|item| item.is_none());
            if let Some(target) = slot {
                *target = Some(Edge { x_start, x_end, y_fp, step });
            }
        }

        for x in minx..=maxx {
            let mut y_min_fp: i64 = i64::MAX;
            let mut y_max_fp: i64 = i64::MIN;
            let mut hits: i32 = 0;

            for edge in edges.iter_mut().flatten() {
                if x < edge.x_start || x >= edge.x_end {
                    continue;
                }
                let y_fp = edge.y_fp;
                y_min_fp = y_min_fp.min(y_fp);
                y_max_fp = y_max_fp.max(y_fp);
                edge.y_fp = edge.y_fp.saturating_add(edge.step);
                hits += 1;
            }

            if hits < 2 {
                continue;
            }
            let y0 = ((y_min_fp + 0xFFFF) >> 16) as i32;
            let y1_excl = ((y_max_fp >> 16) as i32) + 1;
            self.fill_col_span(x, y0, y1_excl, r, g, bcol);
        }
    }

    unsafe fn fill_tris_solid(&self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8) {
        let mut i = 0usize;
        while i + 2 < indices.len() {
            let ia = indices[i] as usize;
            let ib = indices[i + 1] as usize;
            let ic = indices[i + 2] as usize;
            i += 3;

            if ia >= verts.len() || ib >= verts.len() || ic >= verts.len() { continue; }
            self.fill_triangle_solid(verts[ia], verts[ib], verts[ic], tx, ty, r, g, b);
        }
    }

    #[inline(always)]
    unsafe fn draw_line(&self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, r: u8, g: u8, b: u8) {
        // Bresenham
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            self.put_pixel(x0, y0, r, g, b);
            if x0 == x1 && y0 == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    unsafe fn draw_tris_wireframe(&self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8) {
        let mut i = 0usize;
        while i + 2 < indices.len() {
            let ia = indices[i] as usize;
            let ib = indices[i + 1] as usize;
            let ic = indices[i + 2] as usize;
            i += 3;

            if ia >= verts.len() || ib >= verts.len() || ic >= verts.len() { continue; }
            let a = verts[ia]; let b0 = verts[ib]; let c = verts[ic];
            let ax = a.x + tx; let ay = a.y + ty;
            let bx = b0.x + tx; let by = b0.y + ty;
            let cx = c.x + tx; let cy = c.y + ty;

            self.draw_line(ax, ay, bx, by, r, g, b);
            self.draw_line(bx, by, cx, cy, r, g, b);
            self.draw_line(cx, cy, ax, ay, r, g, b);
        }
    }
}

fn top_left_fb() -> Option<FbView> {
    let mut w: u16 = 0;
    let mut h: u16 = 0;
    let ptr = unsafe { gfxGetFramebuffer(GFX_TOP, GFX_LEFT, &mut w, &mut h) };
    if ptr.is_null() || w == 0 || h == 0 { return None; }
    Some(FbView { ptr, w_mem: w as usize, h_mem: h as usize, scissor: None })
}

/// 3DS framebuffer-backed device.
///
/// Design rule: this is the ONLY module allowed to touch `gfxGetFramebuffer` or raw framebuffer pointers.
pub struct Fb3dsDevice {
    fb: Option<FbView>,
    scissor: Option<RectI>,
}

impl Fb3dsDevice {
    pub fn new() -> Self {
        Self { fb: None, scissor: None }
    }
}

impl RenderDevice for Fb3dsDevice {
    fn surface_width(&self) -> i32 {
        self.fb.map(|fb| fb.disp_w() as i32).unwrap_or(400)
    }

    fn surface_height(&self) -> i32 {
        self.fb.map(|fb| fb.disp_h() as i32).unwrap_or(240)
    }

    fn begin_frame(&mut self) {
        self.fb = top_left_fb().map(|mut fb| {
            fb.scissor = self.scissor;
            fb
        });
    }

    fn end_frame(&mut self) {
        // No swap/flush here; C-side owns presentation.
        self.fb = None;
    }

    fn clear(&mut self, clear: ClearColor) {
        if let Some(fb) = self.fb {
            unsafe { fb.clear(clear.r, clear.g, clear.b); }
        }
    }

    fn fill_rect(&mut self, rect: RectI, r: u8, g: u8, b: u8) {
        if let Some(fb) = self.fb {
            unsafe { fb.fill_rect(rect.x, rect.y, rect.w, rect.h, r, g, b); }
        }
    }

    fn stroke_rect(&mut self, rect: RectI, r: u8, g: u8, b: u8) {
        if let Some(fb) = self.fb {
            // Draw a 1px outline. Use inclusive end points.
            let w = rect.w;
            let h = rect.h;
            if w <= 0 || h <= 0 {
                return;
            }
            let x0 = rect.x;
            let y0 = rect.y;
            let x1 = rect.x + w - 1;
            let y1 = rect.y + h - 1;
            unsafe {
                fb.draw_line(x0, y0, x1, y0, r, g, b);
                fb.draw_line(x0, y1, x1, y1, r, g, b);
                fb.draw_line(x0, y0, x0, y1, r, g, b);
                fb.draw_line(x1, y0, x1, y1, r, g, b);
            }
        }
    }

    fn blit_rgba(&mut self, x: i32, y: i32, src: &BitmapSurface) {
        if let Some(fb) = self.fb {
            unsafe { fb.blit_rgba(x, y, src); }
        }
    }

    fn set_scissor(&mut self, rect: Option<RectI>) {
        self.scissor = rect;
        if let Some(mut fb) = self.fb {
            fb.scissor = rect;
            self.fb = Some(fb);
        }
    }

    fn draw_tris_textured(
        &mut self,
        verts: &[TexVertex],
        indices: &[u16],
        src: &BitmapSurface,
        color_transform: Option<ColorTransform>,
    ) {
        if let Some(fb) = self.fb {
            if verts.is_empty() || indices.len() < 3 {
                return;
            }
            for tri in indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }
                let ia = tri[0] as usize;
                let ib = tri[1] as usize;
                let ic = tri[2] as usize;
                if ia >= verts.len() || ib >= verts.len() || ic >= verts.len() {
                    continue;
                }
                unsafe {
                    fb.draw_triangle_textured(verts[ia], verts[ib], verts[ic], src, color_transform);
                }
            }
        }
    }

    fn fill_tris_solid(&mut self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8) {
        if let Some(fb) = self.fb {
            unsafe { fb.fill_tris_solid(verts, indices, tx, ty, r, g, b); }
        }
    }

    fn draw_tris_wireframe(&mut self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8) {
        if let Some(fb) = self.fb {
            unsafe { fb.draw_tris_wireframe(verts, indices, tx, ty, r, g, b); }
        }
    }

}

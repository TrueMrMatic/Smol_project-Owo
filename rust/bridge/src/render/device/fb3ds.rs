use crate::render::device::RenderDevice;
use crate::render::frame::{ClearColor, RectI};
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
}

impl FbView {
    #[inline(always)]
    fn disp_w(&self) -> usize { self.h_mem }
    #[inline(always)]
    fn disp_h(&self) -> usize { self.w_mem }

    #[inline(always)]
    unsafe fn put_pixel(&self, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if x < 0 || y < 0 { return; }
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

                    if sa == 255 {
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
        let cy0 = y0.max(0);
        let cy1 = y1_excl.min(self.disp_h() as i32);
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

        // Edge list
        let vx = [ax, bx, cx];
        let vy = [ay, by, cy];

        for x in minx..=maxx {
            // Track min/max intersection y in 16.16 fixed-point.
            let mut y_min_fp: i64 = i64::MAX;
            let mut y_max_fp: i64 = i64::MIN;
            let mut hits: i32 = 0;

            for e in 0..3 {
                let j = (e + 1) % 3;
                let x0 = vx[e];
                let y0 = vy[e];
                let x1 = vx[j];
                let y1 = vy[j];

                if x0 == x1 {
                    // Vertical edge: if we are on the same x, add both endpoints.
                    if x == x0 {
                        let y0fp = (y0 as i64) << 16;
                        let y1fp = (y1 as i64) << 16;
                        y_min_fp = y_min_fp.min(y0fp.min(y1fp));
                        y_max_fp = y_max_fp.max(y0fp.max(y1fp));
                        hits += 2;
                    }
                    continue;
                }

                let ex0 = x0.min(x1);
                let ex1 = x0.max(x1);

                // Half-open rule in X to avoid double hits on shared edges.
                if x < ex0 || x >= ex1 { continue; }

                let dx = (x1 - x0) as i64;
                let dy = (y1 - y0) as i64;
                let t_num = (x - x0) as i64;

                let y_fp = ((y0 as i64) << 16) + (t_num * dy << 16) / dx;
                y_min_fp = y_min_fp.min(y_fp);
                y_max_fp = y_max_fp.max(y_fp);
                hits += 1;
            }

            if hits < 2 { continue; }

            // Convert to integer span [y0, y1_excl)
            let y0 = ((y_min_fp + 0xFFFF) >> 16) as i32; // ceil(min)
            let y1_excl = ((y_max_fp >> 16) as i32) + 1; // floor(max)+1
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
    Some(FbView { ptr, w_mem: w as usize, h_mem: h as usize })
}

/// 3DS framebuffer-backed device.
///
/// Design rule: this is the ONLY module allowed to touch `gfxGetFramebuffer` or raw framebuffer pointers.
pub struct Fb3dsDevice {
    fb: Option<FbView>,
}

impl Fb3dsDevice {
    pub fn new() -> Self {
        Self { fb: None }
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
        self.fb = top_left_fb();
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

pub mod fb3ds;

use crate::render::frame::{ClearColor, RectI};
use crate::render::cache::bitmaps::BitmapSurface;
use crate::render::cache::shapes::Vertex2;

/// Platform drawing interface.
///
/// Design rule: only `render/device/*` can touch platform APIs.
pub trait RenderDevice {
    /// Display surface width in pixels.
    fn surface_width(&self) -> i32;

    /// Display surface height in pixels.
    fn surface_height(&self) -> i32;

    fn clear(&mut self, clear: ClearColor);
    fn fill_rect(&mut self, rect: RectI, r: u8, g: u8, b: u8);

    /// Draw a 1px outline of `rect` (used for wireframe/debug overlays).
    fn stroke_rect(&mut self, rect: RectI, r: u8, g: u8, b: u8);

    /// Draw an RGBA8 bitmap at `(x, y)`.
    ///
    /// Step 3 bootstrap: no scaling, nearest sampling, basic alpha blending.
    fn blit_rgba(&mut self, x: i32, y: i32, src: &BitmapSurface);

    /// Fill a set of triangles with an opaque solid color.
    ///
    /// `verts` are in shape-local pixel units; `(tx, ty)` is a per-draw translation applied by the device.
    fn fill_tris_solid(&mut self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8);

    /// Optional debug: draw triangle edges (wireframe).
    fn draw_tris_wireframe(&mut self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8);


    /// Called at the beginning of each frame.
    fn begin_frame(&mut self);

    /// Called at the end of each frame.
    fn end_frame(&mut self);
}

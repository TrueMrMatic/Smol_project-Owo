pub mod fb3ds;

use crate::render::frame::{ClearColor, ColorTransform, Matrix2D, RectI, TexVertex};
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

    /// Set or clear a scissor rectangle for masking.
    fn set_scissor(&mut self, rect: Option<RectI>);

    /// Draw textured triangles with nearest-neighbor sampling.
    fn draw_tris_textured(
        &mut self,
        verts: &[TexVertex],
        indices: &[u16],
        src: &BitmapSurface,
        color_transform: Option<ColorTransform>,
    );

    /// Fill a set of triangles with an opaque solid color.
    ///
    /// `verts` are in shape-local pixel units; `(tx, ty)` is a per-draw translation applied by the device.
    fn fill_tris_solid(&mut self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8);

    /// Optional debug: draw triangle edges (wireframe).
    fn draw_tris_wireframe(&mut self, verts: &[Vertex2], indices: &[u16], tx: i32, ty: i32, r: u8, g: u8, b: u8);

    /// Fill a set of triangles with an opaque solid color and an affine transform.
    ///
    /// The default implementation fast-paths translation-only matrices.
    fn fill_tris_solid_affine(
        &mut self,
        verts: &[Vertex2],
        indices: &[u16],
        transform: Matrix2D,
        r: u8,
        g: u8,
        b: u8,
    ) {
        if transform.is_translation() {
            let tx = transform.tx.round() as i32;
            let ty = transform.ty.round() as i32;
            self.fill_tris_solid(verts, indices, tx, ty, r, g, b);
        } else {
            let tx = transform.tx.round() as i32;
            let ty = transform.ty.round() as i32;
            self.fill_tris_solid(verts, indices, tx, ty, r, g, b);
        }
    }

    /// Optional debug: draw triangle edges (wireframe) with an affine transform.
    ///
    /// The default implementation fast-paths translation-only matrices.
    fn draw_tris_wireframe_affine(
        &mut self,
        verts: &[Vertex2],
        indices: &[u16],
        transform: Matrix2D,
        r: u8,
        g: u8,
        b: u8,
    ) {
        if transform.is_translation() {
            let tx = transform.tx.round() as i32;
            let ty = transform.ty.round() as i32;
            self.draw_tris_wireframe(verts, indices, tx, ty, r, g, b);
        } else {
            let tx = transform.tx.round() as i32;
            let ty = transform.ty.round() as i32;
            self.draw_tris_wireframe(verts, indices, tx, ty, r, g, b);
        }
    }

    /// Called at the beginning of each frame.
    fn begin_frame(&mut self);

    /// Called at the end of each frame.
    fn end_frame(&mut self);
}

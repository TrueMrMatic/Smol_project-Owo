use ruffle_core::Color;

#[derive(Clone, Copy, Debug)]
pub struct ClearColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ClearColor {
    pub fn from_ruffle(c: Color) -> Self {
        Self { r: c.r, g: c.g, b: c.b }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Matrix2D {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Matrix2D {
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty)
    }

    pub fn is_identity(&self) -> bool {
        approx_eq_f32(self.a, 1.0)
            && approx_eq_f32(self.d, 1.0)
            && approx_eq_f32(self.b, 0.0)
            && approx_eq_f32(self.c, 0.0)
            && approx_eq_f32(self.tx, 0.0)
            && approx_eq_f32(self.ty, 0.0)
    }

    pub fn is_axis_aligned(&self) -> bool {
        approx_eq_f32(self.b, 0.0) && approx_eq_f32(self.c, 0.0)
    }

    pub fn is_translation(&self) -> bool {
        approx_eq_f32(self.a, 1.0)
            && approx_eq_f32(self.d, 1.0)
            && approx_eq_f32(self.b, 0.0)
            && approx_eq_f32(self.c, 0.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TexUvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl TexUvRect {
    pub fn full() -> Self {
        Self { u0: 0.0, v0: 0.0, u1: 1.0, v1: 1.0 }
    }

    pub fn is_full(&self) -> bool {
        approx_eq_f32(self.u0, 0.0)
            && approx_eq_f32(self.v0, 0.0)
            && approx_eq_f32(self.u1, 1.0)
            && approx_eq_f32(self.v1, 1.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ColorTransform {
    pub mul: [f32; 4],
    pub add: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct TexVertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
}

#[derive(Clone, Debug)]
pub struct FramePacket {
    pub clear: ClearColor,
    pub cmds: Vec<RenderCmd>,
}

impl FramePacket {
    pub fn new() -> Self {
        Self {
            clear: ClearColor { r: 0, g: 0, b: 0 },
            cmds: Vec::new(),
        }
    }

    pub fn reset(&mut self, clear: Color) {
        self.clear = ClearColor::from_ruffle(clear);
        self.cmds.clear();
    }
}

#[derive(Clone, Debug)]
pub enum RenderCmd {
    /// Bootstrap shape rendering: draw a cached bounds-rect with a stable per-shape color.
    FillRect {
        rect: RectI,
        color_key: u64,
        /// If true, draw a thin white outline after filling.
        ///
        /// Used for debugging (L-hold wireframe overlay) and for fallback when tessellation fails.
        wireframe: bool,
    },

    /// Step 2A: draw one cached fill-mesh for a shape (opaque fill only).
    ///
    /// Shapes commonly contain multiple fills. We store meshes per fill at registration time and
    /// emit one draw command per fill.
    ///
    /// The executor will look up the mesh by `(shape_key, fill_idx)` in `SharedCaches.shapes`.
    /// The full affine `transform` is applied at draw time (no per-frame allocations).
    DrawShapeSolidFill {
        shape_key: usize,
        fill_idx: u16,
        transform: Matrix2D,
        solid_rgba: Option<[u8; 4]>,
        color_transform: Option<ColorTransform>,
        color_key: u64,
        wireframe: bool,
    },

    /// Text glyph fill (vector outlines).
    DrawTextSolidFill {
        shape_key: usize,
        fill_idx: u16,
        transform: Matrix2D,
        solid_rgba: Option<[u8; 4]>,
        color_transform: Option<ColorTransform>,
        color_key: u64,
        wireframe: bool,
    },

    /// Step 2B: draw one cached stroke mesh for a shape (constant width).
    DrawShapeStroke {
        shape_key: usize,
        stroke_idx: u16,
        transform: Matrix2D,
        r: u8,
        g: u8,
        b: u8,
        wireframe: bool,
    },

    /// Push a rectangular mask (scissor).
    PushMaskRect {
        rect: RectI,
    },

    /// Push a shape mask (not yet supported; will warn + no-op).
    PushMaskShape {
        shape_key: usize,
        tx: i32,
        ty: i32,
    },

    /// Pop the most recent mask.
    PopMask,

    /// Draw a cached RGBA bitmap at `(x, y)` without scaling (nearest).
    ///
    /// Step 3 will extend this to support transforms (scale/rotation) and
    /// textured triangles.
    BlitBitmap {
        bitmap_key: usize,
        transform: Matrix2D,
        uv: TexUvRect,
        color_transform: Option<ColorTransform>,
    },

    /// Visual cue until we see real draw commands.
    DebugLoadingIndicator,

    /// Developer overlay: draw a known affine-transformed rectangle mesh.
    DebugAffineRect {
        transform: Matrix2D,
        r: u8,
        g: u8,
        b: u8,
    },

}

fn approx_eq_f32(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.0001
}

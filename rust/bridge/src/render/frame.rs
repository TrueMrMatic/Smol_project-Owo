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
    /// Translation `(tx, ty)` is applied at draw time (no per-frame allocations).
    DrawShapeSolidFill {
        shape_key: usize,
        fill_idx: u16,
        tx: i32,
        ty: i32,
        color_key: u64,
        wireframe: bool,
    },

    /// Draw a cached RGBA bitmap at `(x, y)` without scaling (nearest).
    ///
    /// Step 3 will extend this to support transforms (scale/rotation) and
    /// textured triangles.
    BlitBitmap {
        x: i32,
        y: i32,
        bitmap_key: usize,
    },

    /// Visual cue until we see real draw commands.
    DebugLoadingIndicator,

}

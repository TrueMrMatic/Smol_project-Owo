use std::collections::HashMap;

use crate::render::frame::RectI;

pub type ShapeKey = usize;

#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex2 {
    pub x: i32,
    pub y: i32,
}

/// One fill mesh for a shape.
///
/// Design rule: renderer-owned data only (no Ruffle types).
#[derive(Clone, Debug)]
pub struct FillMesh {
    pub verts: Vec<Vertex2>,
    pub indices: Vec<u16>,
}

#[derive(Clone, Debug)]
struct ShapeEntry {
    bounds: RectI,
    fills: Vec<FillMesh>,
    /// True if the entire tessellation failed and we have no triangle mesh.
    tess_failed: bool,
    /// True if tessellation produced some meshes but at least one fill failed.
    tess_partial: bool,
}

/// Cache of registered shapes.
///
/// Design rule: this module contains no Ruffle types.
/// Step 2A stores cached triangle meshes for **fills only**.
pub struct ShapeCache {
    by_key: HashMap<ShapeKey, ShapeEntry>,
}

impl ShapeCache {
    pub fn new() -> Self {
        Self { by_key: HashMap::new() }
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn insert_bounds(&mut self, key: ShapeKey, bounds: RectI) {
        self.by_key.insert(
            key,
            ShapeEntry {
                bounds,
                fills: Vec::new(),
                tess_failed: false,
                tess_partial: false,
            },
        );
    }

    /// Insert only bounds and mark tessellation as failed.
    ///
    /// This allows runtime fallback to the old bounds rectangle while keeping a HUD warning visible
    /// whenever that shape is drawn.
    pub fn insert_bounds_failed(&mut self, key: ShapeKey, bounds: RectI) {
        self.by_key.insert(
            key,
            ShapeEntry {
                bounds,
                fills: Vec::new(),
                tess_failed: true,
                tess_partial: false,
            },
        );
    }

    /// Convenience for bootstrap: insert a 2-triangle rectangle mesh for the given bounds.
    ///
    /// This stays useful as a fast-path in the executor.
    pub fn insert_rect_mesh(&mut self, key: ShapeKey, bounds: RectI) {
        let x0 = bounds.x;
        let y0 = bounds.y;
        let x1 = bounds.x + bounds.w;
        let y1 = bounds.y + bounds.h;

        let verts = vec![
            Vertex2 { x: x0, y: y0 },
            Vertex2 { x: x1, y: y0 },
            Vertex2 { x: x1, y: y1 },
            Vertex2 { x: x0, y: y1 },
        ];
        let indices: Vec<u16> = vec![0, 1, 2, 0, 2, 3];
        let fill = FillMesh { verts, indices };
        self.by_key.insert(
            key,
            ShapeEntry {
                bounds,
                fills: vec![fill],
                tess_failed: false,
                tess_partial: false,
            },
        );
    }

    /// Insert multiple fill meshes for a shape.
    ///
    /// `tess_partial` should be set if some fills failed during tessellation.
    pub fn insert_fill_meshes(
        &mut self,
        key: ShapeKey,
        bounds: RectI,
        fills: Vec<FillMesh>,
        tess_partial: bool,
    ) {
        self.by_key.insert(
            key,
            ShapeEntry {
                bounds,
                fills,
                tess_failed: false,
                tess_partial,
            },
        );
    }

    pub fn get_bounds(&self, key: ShapeKey) -> Option<RectI> {
        self.by_key.get(&key).map(|e| e.bounds)
    }

    pub fn fill_count(&self, key: ShapeKey) -> usize {
        self.by_key.get(&key).map(|e| e.fills.len()).unwrap_or(0)
    }

    pub fn has_mesh(&self, key: ShapeKey) -> bool {
        self.fill_count(key) > 0
    }

    pub fn is_tess_failed(&self, key: ShapeKey) -> bool {
        self.by_key.get(&key).map(|e| e.tess_failed).unwrap_or(false)
    }

    pub fn is_tess_partial(&self, key: ShapeKey) -> bool {
        self.by_key.get(&key).map(|e| e.tess_partial).unwrap_or(false)
    }

    pub fn get_fill_mesh(&self, key: ShapeKey, fill_idx: usize) -> Option<&FillMesh> {
        self.by_key.get(&key).and_then(|e| e.fills.get(fill_idx))
    }

    pub fn get_total_tri_count(&self, key: ShapeKey) -> u32 {
        self.by_key
            .get(&key)
            .map(|e| {
                e.fills
                    .iter()
                    .map(|f| (f.indices.len() as u32) / 3)
                    .sum::<u32>()
            })
            .unwrap_or(0)
    }
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

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
pub struct StrokeMesh {
    pub verts: Vec<Vertex2>,
    pub indices: Vec<u16>,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Debug)]
struct ShapeEntry {
    bounds: RectI,
    fills: Vec<FillMesh>,
    strokes: Vec<StrokeMesh>,
    is_text: bool,
    /// True if the entire tessellation failed and we have no triangle mesh.
    tess_failed: bool,
    /// True if tessellation produced some meshes but at least one fill failed.
    tess_partial: bool,
    stroke_failed: bool,
    stroke_partial: bool,
}

/// Cache of registered shapes.
///
/// Design rule: this module contains no Ruffle types.
/// Step 2A stores cached triangle meshes for **fills only**.
pub struct ShapeCache {
    by_key: HashMap<ShapeKey, ShapeEntry>,
    missing_fill_meshes: AtomicU32,
    invalid_fill_meshes: AtomicU32,
    bounds_fallbacks: AtomicU32,
    missing_stroke_meshes: AtomicU32,
    invalid_stroke_meshes: AtomicU32,
    stroke_bounds_fallbacks: AtomicU32,
}

impl ShapeCache {
    pub fn new() -> Self {
        Self {
            by_key: HashMap::new(),
            missing_fill_meshes: AtomicU32::new(0),
            invalid_fill_meshes: AtomicU32::new(0),
            bounds_fallbacks: AtomicU32::new(0),
            missing_stroke_meshes: AtomicU32::new(0),
            invalid_stroke_meshes: AtomicU32::new(0),
            stroke_bounds_fallbacks: AtomicU32::new(0),
        }
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
                strokes: Vec::new(),
                is_text: false,
                tess_failed: false,
                tess_partial: false,
                stroke_failed: false,
                stroke_partial: false,
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
                strokes: Vec::new(),
                is_text: false,
                tess_failed: true,
                tess_partial: false,
                stroke_failed: true,
                stroke_partial: false,
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
                strokes: Vec::new(),
                is_text: false,
                tess_failed: false,
                tess_partial: false,
                stroke_failed: false,
                stroke_partial: false,
            },
        );
    }

    /// Insert fill/stroke meshes for a shape.
    pub fn insert_meshes(
        &mut self,
        key: ShapeKey,
        bounds: RectI,
        fills: Vec<FillMesh>,
        tess_failed: bool,
        tess_partial: bool,
        strokes: Vec<StrokeMesh>,
        stroke_failed: bool,
        stroke_partial: bool,
        is_text: bool,
    ) {
        self.by_key.insert(
            key,
            ShapeEntry {
                bounds,
                fills,
                strokes,
                is_text,
                tess_failed,
                tess_partial,
                stroke_failed,
                stroke_partial,
            },
        );
    }

    pub fn get_bounds(&self, key: ShapeKey) -> Option<RectI> {
        self.by_key.get(&key).map(|e| e.bounds)
    }

    pub fn fill_count(&self, key: ShapeKey) -> usize {
        self.by_key.get(&key).map(|e| e.fills.len()).unwrap_or(0)
    }

    pub fn stroke_count(&self, key: ShapeKey) -> usize {
        self.by_key.get(&key).map(|e| e.strokes.len()).unwrap_or(0)
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

    pub fn is_stroke_failed(&self, key: ShapeKey) -> bool {
        self.by_key.get(&key).map(|e| e.stroke_failed).unwrap_or(false)
    }

    pub fn is_stroke_partial(&self, key: ShapeKey) -> bool {
        self.by_key.get(&key).map(|e| e.stroke_partial).unwrap_or(false)
    }

    pub fn get_fill_mesh(&self, key: ShapeKey, fill_idx: usize) -> Option<&FillMesh> {
        self.by_key.get(&key).and_then(|e| e.fills.get(fill_idx))
    }

    pub fn get_stroke_mesh(&self, key: ShapeKey, stroke_idx: usize) -> Option<&StrokeMesh> {
        self.by_key.get(&key).and_then(|e| e.strokes.get(stroke_idx))
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

    pub fn is_text_shape(&self, key: ShapeKey) -> bool {
        self.by_key.get(&key).map(|e| e.is_text).unwrap_or(false)
    }

    pub fn record_missing_fill_mesh(&self) {
        self.missing_fill_meshes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_invalid_fill_mesh(&self) {
        self.invalid_fill_meshes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_bounds_fallback(&self) {
        self.bounds_fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_missing_stroke_mesh(&self) {
        self.missing_stroke_meshes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_invalid_stroke_mesh(&self) {
        self.invalid_stroke_meshes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_stroke_bounds_fallback(&self) {
        self.stroke_bounds_fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn stats(&self) -> (u32, u32, u32) {
        (
            self.missing_fill_meshes.load(Ordering::Relaxed),
            self.invalid_fill_meshes.load(Ordering::Relaxed),
            self.bounds_fallbacks.load(Ordering::Relaxed),
        )
    }

    pub fn stroke_stats(&self) -> (u32, u32, u32) {
        (
            self.missing_stroke_meshes.load(Ordering::Relaxed),
            self.invalid_stroke_meshes.load(Ordering::Relaxed),
            self.stroke_bounds_fallbacks.load(Ordering::Relaxed),
        )
    }
}

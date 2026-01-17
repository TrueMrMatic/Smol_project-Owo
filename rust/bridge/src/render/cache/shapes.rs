use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::render::frame::RectI;
use crate::runlog;

pub type ShapeKey = usize;

#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex2 {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug)]
pub enum FillPaint {
    SolidRGBA(u8, u8, u8, u8),
    Unsupported,
}

/// One fill mesh for a shape.
///
/// Design rule: renderer-owned data only (no Ruffle types).
#[derive(Clone, Debug)]
pub struct FillMesh {
    pub verts: Vec<Vertex2>,
    pub indices: Vec<u16>,
    pub paint: FillPaint,
}

#[derive(Clone, Debug)]
pub struct StrokeMesh {
    pub verts: Vec<Vertex2>,
    pub indices: Vec<u16>,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug)]
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
    bytes_estimate: usize,
    debug_id: u32,
    last_used: AtomicU32,
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
    bytes_used: usize,
    budget_bytes: usize,
    lru_clock: AtomicU32,
    evicted_entries: AtomicU32,
    evicted_bytes: AtomicU32,
}

impl ShapeCache {
    pub fn new() -> Self {
        const SHAPE_CACHE_BUDGET_BYTES: usize = 8 * 1024 * 1024;
        Self {
            by_key: HashMap::new(),
            missing_fill_meshes: AtomicU32::new(0),
            invalid_fill_meshes: AtomicU32::new(0),
            bounds_fallbacks: AtomicU32::new(0),
            missing_stroke_meshes: AtomicU32::new(0),
            invalid_stroke_meshes: AtomicU32::new(0),
            stroke_bounds_fallbacks: AtomicU32::new(0),
            bytes_used: 0,
            budget_bytes: SHAPE_CACHE_BUDGET_BYTES,
            lru_clock: AtomicU32::new(0),
            evicted_entries: AtomicU32::new(0),
            evicted_bytes: AtomicU32::new(0),
        }
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
        self.bytes_used = 0;
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn touch(&self, key: ShapeKey) {
        let clock = self.lru_clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if let Some(entry) = self.by_key.get(&key) {
            entry.last_used.store(clock, Ordering::Relaxed);
        }
    }

    pub fn insert_bounds(&mut self, key: ShapeKey, bounds: RectI) {
        let clock = self.lru_clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let entry = ShapeEntry {
            bounds,
            fills: Vec::new(),
            strokes: Vec::new(),
            is_text: false,
            tess_failed: false,
            tess_partial: false,
            stroke_failed: false,
            stroke_partial: false,
            bytes_estimate: 0,
            debug_id: 0,
            last_used: AtomicU32::new(clock),
        };
        self.insert_entry(key, entry);
    }

    /// Insert only bounds and mark tessellation as failed.
    ///
    /// This allows runtime fallback to the old bounds rectangle while keeping a HUD warning visible
    /// whenever that shape is drawn.
    pub fn insert_bounds_failed(&mut self, key: ShapeKey, bounds: RectI) {
        let clock = self.lru_clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let entry = ShapeEntry {
            bounds,
            fills: Vec::new(),
            strokes: Vec::new(),
            is_text: false,
            tess_failed: true,
            tess_partial: false,
            stroke_failed: true,
            stroke_partial: false,
            bytes_estimate: 0,
            debug_id: 0,
            last_used: AtomicU32::new(clock),
        };
        self.insert_entry(key, entry);
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
        let fill = FillMesh { verts, indices, paint: FillPaint::Unsupported };
        let fills = vec![fill];
        let bytes_estimate = estimate_mesh_bytes(&fills, &[]);
        let clock = self.lru_clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let entry = ShapeEntry {
            bounds,
            fills,
            strokes: Vec::new(),
            is_text: false,
            tess_failed: false,
            tess_partial: false,
            stroke_failed: false,
            stroke_partial: false,
            bytes_estimate,
            debug_id: 0,
            last_used: AtomicU32::new(clock),
        };
        self.insert_entry(key, entry);
    }

    /// Insert fill/stroke meshes for a shape.
    pub fn insert_meshes(
        &mut self,
        key: ShapeKey,
        debug_id: u32,
        bounds: RectI,
        fills: Vec<FillMesh>,
        tess_failed: bool,
        tess_partial: bool,
        strokes: Vec<StrokeMesh>,
        stroke_failed: bool,
        stroke_partial: bool,
        is_text: bool,
    ) {
        let bytes_estimate = estimate_mesh_bytes(&fills, &strokes);
        if bytes_estimate > (self.budget_bytes / 2) {
            runlog::warn_line(&format!(
                "shape_cache_oversize_drop id={} bytes={} budget={}",
                debug_id,
                bytes_estimate,
                self.budget_bytes
            ));
            let clock = self.lru_clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
            let entry = ShapeEntry {
                bounds,
                fills: Vec::new(),
                strokes: Vec::new(),
                is_text,
                tess_failed: true,
                tess_partial: false,
                stroke_failed: true,
                stroke_partial: false,
                bytes_estimate: 0,
                debug_id,
                last_used: AtomicU32::new(clock),
            };
            self.insert_entry(key, entry);
            return;
        }

        let clock = self.lru_clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let entry = ShapeEntry {
            bounds,
            fills,
            strokes,
            is_text,
            tess_failed,
            tess_partial,
            stroke_failed,
            stroke_partial,
            bytes_estimate,
            debug_id,
            last_used: AtomicU32::new(clock),
        };
        self.insert_entry(key, entry);
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

    pub fn mem_stats(&self) -> (usize, usize, u32, u32) {
        (
            self.bytes_used,
            self.budget_bytes,
            self.evicted_entries.load(Ordering::Relaxed),
            self.evicted_bytes.load(Ordering::Relaxed),
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

fn estimate_mesh_bytes(fills: &[FillMesh], strokes: &[StrokeMesh]) -> usize {
    let fill_bytes: usize = fills
        .iter()
        .map(|mesh| {
            mesh.verts.len() * size_of::<Vertex2>() + mesh.indices.len() * size_of::<u16>()
        })
        .sum();
    let stroke_bytes: usize = strokes
        .iter()
        .map(|mesh| {
            mesh.verts.len() * size_of::<Vertex2>() + mesh.indices.len() * size_of::<u16>()
        })
        .sum();
    fill_bytes + stroke_bytes
}

impl ShapeCache {
    fn insert_entry(&mut self, key: ShapeKey, entry: ShapeEntry) {
        let bytes_estimate = entry.bytes_estimate;
        if let Some(prev) = self.by_key.insert(key, entry) {
            self.bytes_used = self.bytes_used.saturating_sub(prev.bytes_estimate);
        }
        self.bytes_used = self.bytes_used.saturating_add(bytes_estimate);
        self.evict_if_needed();
    }

    fn evict_if_needed(&mut self) {
        let mut logged = false;
        while self.bytes_used > self.budget_bytes {
            let mut oldest_key: Option<ShapeKey> = None;
            let mut oldest_used = u32::MAX;
            for (key, entry) in &self.by_key {
                let used = entry.last_used.load(Ordering::Relaxed);
                if used < oldest_used {
                    oldest_used = used;
                    oldest_key = Some(*key);
                }
            }

            let Some(key) = oldest_key else {
                break;
            };

            if let Some(entry) = self.by_key.remove(&key) {
                self.bytes_used = self.bytes_used.saturating_sub(entry.bytes_estimate);
                self.evicted_entries.fetch_add(1, Ordering::Relaxed);
                self.evicted_bytes.fetch_add(entry.bytes_estimate as u32, Ordering::Relaxed);
                if !logged {
                    logged = true;
                    runlog::log_important(&format!(
                        "shape_cache_evict key={} id={} bytes={} used={} budget={}",
                        key,
                        entry.debug_id,
                        entry.bytes_estimate,
                        self.bytes_used,
                        self.budget_bytes
                    ));
                }
            } else {
                break;
            }
        }
    }
}

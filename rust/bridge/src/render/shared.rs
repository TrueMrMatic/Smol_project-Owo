use std::sync::{Arc, Mutex};

use crate::render::cache::bitmaps::BitmapCache;
use crate::render::cache::shapes::ShapeCache;

/// Shared CPU-side render resources.
///
/// The Ruffle adapter populates these caches when handles are registered
/// (`register_bitmap`, `register_shape` tessellation, etc.).
/// The renderer consumes them when executing `FramePacket` commands.
///
/// Design rule: This module contains no Ruffle types.
#[derive(Clone)]
pub struct SharedCaches {
    pub bitmaps: Arc<Mutex<BitmapCache>>,
    pub shapes: Arc<Mutex<ShapeCache>>,
}

impl SharedCaches {
    pub fn new() -> Self {
        Self {
            bitmaps: Arc::new(Mutex::new(BitmapCache::new())),
            shapes: Arc::new(Mutex::new(ShapeCache::new())),
        }
    }
}

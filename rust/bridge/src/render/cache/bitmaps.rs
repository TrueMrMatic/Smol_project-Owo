use std::collections::HashMap;

/// A stable key for bitmap handles.
///
/// We use the address of the handle's Arc allocation (via `Arc::as_ptr`) so
/// lookups are O(1) and we don't need to depend on Ruffle internals here.
pub type BitmapKey = usize;

/// CPU-side bitmap surface in RGBA8.
///
/// - `rgba` is row-major, 4 bytes per pixel (R,G,B,A).
/// - Alpha is *straight* (not pre-multiplied). If upstream provides premultiplied
///   data, blending will look darker; we'll correct once we confirm the format.
#[derive(Clone, Debug)]
pub struct BitmapSurface {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl BitmapSurface {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self { width, height, rgba }
    }

    pub fn is_valid(&self) -> bool {
        let px = self.width as usize * self.height as usize;
        self.rgba.len() == px * 4
    }
}

/// Cache of registered bitmaps.
///
/// Step 3 will evolve this into a more explicit "upload" cache and later into
/// a GPU texture cache (Citro2D/Citro3D).
pub struct BitmapCache {
    by_key: HashMap<BitmapKey, BitmapSurface>,
}

impl BitmapCache {
    pub fn new() -> Self {
        Self { by_key: HashMap::new() }
    }

    pub fn clear(&mut self) {
        self.by_key.clear();
    }

    pub fn insert(&mut self, key: BitmapKey, surface: BitmapSurface) {
        self.by_key.insert(key, surface);
    }

    pub fn get(&self, key: BitmapKey) -> Option<&BitmapSurface> {
        self.by_key.get(&key)
    }

    pub fn contains_key(&self, key: BitmapKey) -> bool {
        self.by_key.contains_key(&key)
    }

    pub fn get_mut(&mut self, key: BitmapKey) -> Option<&mut BitmapSurface> {
        self.by_key.get_mut(&key)
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }
}

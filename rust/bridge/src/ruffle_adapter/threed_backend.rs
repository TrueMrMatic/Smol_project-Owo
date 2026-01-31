use std::borrow::Cow;
use core::future::Future;
use core::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "net")]
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
#[cfg(feature = "net")]
use std::time::Duration;
use std::time::Instant;

#[cfg(feature = "net")]
use async_channel::{Receiver, Sender};
#[cfg(feature = "net")]
use indexmap::IndexMap;
use url::Url;

use ruffle_core::backend::log::LogBackend;
#[cfg(feature = "net")]
use ruffle_core::backend::navigator::{NavigatorBackend, NavigationMethod, Request, SuccessResponse, ErrorResponse};
#[cfg(feature = "storage")]
use ruffle_core::backend::storage::StorageBackend;
use ruffle_core::backend::ui::{UiBackend, MouseCursor, FileFilter, FileDialogResult, FontDefinition, LanguageIdentifier, DialogLoaderError};
use ruffle_core::font::FontQuery;
#[cfg(feature = "net")]
use ruffle_core::socket::{SocketAction, SocketHandle};
use ruffle_core::Color;

use ruffle_render::backend::{
    RenderBackend, ViewportDimensions, Context3D, Context3DProfile,
    ShapeHandle, ShapeHandleImpl, PixelBenderOutput, PixelBenderTarget, BitmapCacheEntry,
};
use ruffle_render::bitmap::{Bitmap, BitmapHandle, SyncHandle, BitmapSource, PixelRegion, RgbaBufRead, BitmapHandleImpl};
use ruffle_render::commands::{CommandList, Command};
use ruffle_render::error::Error as RenderError;
use ruffle_render::quality::StageQuality;
use ruffle_render::shape_utils::{DistilledShape, DrawPath};
use ruffle_render::pixel_bender::{PixelBenderShader, PixelBenderShaderHandle};
use ruffle_render::pixel_bender_support::PixelBenderShaderArgument;

use crate::render::{ColorTransform, FramePacket, Matrix2D, RenderCmd, RectI, SharedCaches, TexUvRect};
use crate::render::cache::shapes::{FillMesh, FillPaint, Vertex2};
use crate::render::cache::bitmaps::BitmapSurface;
use ruffle_core::swf::ColorTransform as SwfColorTransform;

// Step 2A tessellator lives next to this backend inside ruffle_adapter/.
use super::tessellate;
use crate::runlog;
type ShapeKey = usize;

fn shape_handle_from_impl<T: ShapeHandleImpl + 'static>(handle: Arc<T>) -> ShapeHandle {
    let handle: Arc<dyn ShapeHandleImpl> = handle;
    ShapeHandle(handle)
}

const MAX_TRIS_PER_FRAME: u32 = 8000;
const MAX_UNSUPPORTED_FILL_WARNINGS: u32 = 8;
const SHAPE_WATCHDOG_MS: u64 = 15;
static UNSUPPORTED_FILL_DRAW_WARNINGS: AtomicU32 = AtomicU32::new(0);

fn to_color_transform(ct: SwfColorTransform) -> Option<ColorTransform> {
    if ct == SwfColorTransform::IDENTITY {
        return None;
    }
    let mul = ct.mult_rgba_normalized();
    let add_norm = ct.add_rgba_normalized();
    Some(ColorTransform {
        mul,
        add: [
            add_norm[0] * 255.0,
            add_norm[1] * 255.0,
            add_norm[2] * 255.0,
            add_norm[3] * 255.0,
        ],
    })
}

fn debug_color_from_key(mut k: u64) -> (u8, u8, u8) {
    k = k.wrapping_mul(0x9E3779B185EBCA87);
    k ^= k >> 33;
    k = k.wrapping_mul(0xC2B2AE3D27D4EB4F);
    k ^= k >> 29;
    let r = (k & 0xFF) as u8;
    let g = ((k >> 8) & 0xFF) as u8;
    let b = ((k >> 16) & 0xFF) as u8;
    (r, g, b)
}

fn rect_aabb_transformed(rect: RectI, transform: Matrix2D) -> RectI {
    let x0 = rect.x as f32;
    let y0 = rect.y as f32;
    let x1 = (rect.x + rect.w) as f32;
    let y1 = (rect.y + rect.h) as f32;

    if transform.is_axis_aligned() {
        let tx0 = transform.a * x0 + transform.tx;
        let tx1 = transform.a * x1 + transform.tx;
        let ty0 = transform.d * y0 + transform.ty;
        let ty1 = transform.d * y1 + transform.ty;
        let minx = tx0.min(tx1);
        let maxx = tx0.max(tx1);
        let miny = ty0.min(ty1);
        let maxy = ty0.max(ty1);
        let x = minx.floor() as i32;
        let y = miny.floor() as i32;
        let w = (maxx.ceil() as i32).saturating_sub(x);
        let h = (maxy.ceil() as i32).saturating_sub(y);
        return RectI { x, y, w, h };
    }

    let (tx0, ty0) = transform.apply(x0, y0);
    let (tx1, ty1) = transform.apply(x1, y0);
    let (tx2, ty2) = transform.apply(x1, y1);
    let (tx3, ty3) = transform.apply(x0, y1);

    let minx = tx0.min(tx1.min(tx2.min(tx3)));
    let maxx = tx0.max(tx1.max(tx2.max(tx3)));
    let miny = ty0.min(ty1.min(ty2.min(ty3)));
    let maxy = ty0.max(ty1.max(ty2.max(ty3)));

    let x = minx.floor() as i32;
    let y = miny.floor() as i32;
    let w = (maxx.ceil() as i32).saturating_sub(x);
    let h = (maxy.ceil() as i32).saturating_sub(y);
    RectI { x, y, w, h }
}

fn is_text_shape(shape: &DistilledShape<'_>) -> bool {
    shape.id == 0 && shape.paths.iter().all(|p| matches!(p, DrawPath::Fill { .. }))
}

fn bitmap_to_surface(bitmap: Bitmap) -> BitmapSurface {
    // Ruffle's Bitmap is expected to carry uncompressed RGBA8 pixels.
    // If the layout ever changes, we fall back to a visible magenta pattern.
    // Recent Ruffle versions expose dimensions via methods.
    let width = bitmap.width();
    let height = bitmap.height();
    let mut rgba: Vec<u8> = bitmap.data().to_vec();
    let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
    if rgba.len() != expected {
        rgba = vec![0u8; expected];
        // Magenta checker to make the failure visible on-device.
        for y in 0..(height as usize) {
            for x in 0..(width as usize) {
                let i = 4 * (y * (width as usize) + x);
                let on = ((x ^ y) & 8) != 0;
                rgba[i + 0] = if on { 255 } else { 0 };
                rgba[i + 1] = 0;
                rgba[i + 2] = if on { 255 } else { 0 };
                rgba[i + 3] = 255;
            }
        }
    }
    let mut is_opaque = true;
    for alpha in rgba.iter().skip(3).step_by(4) {
        if *alpha != 255 {
            is_opaque = false;
            break;
        }
    }
    BitmapSurface { width, height, rgba, is_opaque }
}

#[cfg(feature = "net")]
type BoxedFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

#[derive(Default, Clone)]
struct Diagnostics {
    movie_loaded: bool,
    swf_version: u8,
    shapes_registered: u32,
    bitmaps_registered: u32,
    frames_submitted: u32,
    last_cmds_total: u32,
    last_cmds_shapes: u32,
    last_cmds_bitmaps: u32,
    last_cmds_other: u32,
    last_tris: u32,
    total_tess_ms_fills: u64,
    total_tess_ms_strokes: u64,
    max_tess_ms_single_shape: u64,
    total_group_more_correct: u32,
    total_group_fast: u32,
    total_group_trivial: u32,
    total_unsupported_fill_paints: u32,
    last_warning: Option<String>,
    last_fatal: Option<String>,
    last_input: Option<String>,
    input_counter: u64,
}

struct SharedState {
    frame: FramePacket,
    submit_called: bool,
    seen_real_draw: bool,
    dump_next_frame: bool,
    diagnostics: Diagnostics,
    wireframe_once: bool,
    wireframe_hold: bool,
    debug_affine_overlay: bool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            frame: FramePacket::new(),
            submit_called: false,
            seen_real_draw: false,
            dump_next_frame: false,
            diagnostics: Diagnostics::default(),
            wireframe_once: false,
            wireframe_hold: false,
            debug_affine_overlay: false,
        }
    }
}

#[derive(Clone)]
pub struct ThreeDSBackend {
    #[cfg(feature = "net")]
    tasks: Arc<Mutex<Vec<BoxedFuture>>>,
    shared: Arc<Mutex<SharedState>>,
    next_shape_id: Arc<AtomicU32>,
    next_bitmap_id: Arc<AtomicU32>,
    caches: SharedCaches,
}

impl ThreeDSBackend {
    pub fn new(caches: SharedCaches) -> Self {
        Self {
            #[cfg(feature = "net")]
            tasks: Arc::new(Mutex::new(Vec::new())),
            shared: Arc::new(Mutex::new(SharedState::new())),
            next_shape_id: Arc::new(AtomicU32::new(1)),
            next_bitmap_id: Arc::new(AtomicU32::new(1)),
            caches,
        }
    }

    pub fn poll_tasks(&self) {
        #[cfg(feature = "net")]
        {
        let waker = unsafe { Waker::from_raw(dummy_waker()) };
        let mut cx = Context::from_waker(&waker);

        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain_mut(|fut| fut.as_mut().poll(&mut cx) == Poll::Pending);
        }
    }

    pub fn mark_movie_loaded(&self, swf_version: u8) {
        let mut s = self.shared.lock().unwrap();
        s.diagnostics.movie_loaded = true;
        s.diagnostics.swf_version = swf_version;
    }

    pub fn set_fatal_error(&self, msg: String) {
        let mut s = self.shared.lock().unwrap();
        s.diagnostics.last_fatal = Some(msg);
    }

    pub fn begin_frame(&self) {
        let mut s = self.shared.lock().unwrap();
        s.submit_called = false;
        s.diagnostics.last_warning = None;
    }

    pub fn record_input(&self, text: String) {
        let mut s = self.shared.lock().unwrap();
        s.diagnostics.last_input = Some(text);
        s.diagnostics.input_counter = s.diagnostics.input_counter.wrapping_add(1);
    }

    /// Move the latest frame into `dst` without allocating.
    ///
    /// - If `submit_frame` ran this tick: swaps `dst` with the backend-owned packet.
    /// - Otherwise: clears `dst` and sets its clear color.
    pub fn pull_latest_frame_into(&self, dst: &mut FramePacket, clear: Color) {
        let mut s = self.shared.lock().unwrap();
        if s.submit_called {
            std::mem::swap(dst, &mut s.frame);
            s.submit_called = false;
            // Prepare backend packet for next tick; keep vec capacity.
            s.frame.reset(clear);
        } else {
            dst.reset(clear);
        }
    }

    pub fn request_command_dump(&self) {
        let mut s = self.shared.lock().unwrap();
        s.dump_next_frame = true;
    }

    pub fn toggle_wireframe_once(&self) {
        let mut s = self.shared.lock().unwrap();
        s.wireframe_once = true;
    }

    pub fn set_wireframe_hold(&self, enabled: bool) {
        let mut s = self.shared.lock().unwrap();
        s.wireframe_hold = enabled;
    }

    pub fn toggle_debug_affine_overlay(&self) -> bool {
        let mut s = self.shared.lock().unwrap();
        s.debug_affine_overlay = !s.debug_affine_overlay;
        s.debug_affine_overlay
    }

    pub fn debug_affine_overlay_enabled(&self) -> bool {
        let s = self.shared.lock().unwrap();
        s.debug_affine_overlay
    }

    fn shape_timeout_fallback(
        &mut self,
        key: ShapeKey,
        id: u32,
        bounds: RectI,
        elapsed_ms: u64,
        stage: &str,
        handle_impl: &Arc<ThreeDSShapeHandleImpl>,
    ) -> ShapeHandle {
        runlog::warn_line(&format!(
            "Shape Timeout id={} elapsed_ms={} stage={}",
            id, elapsed_ms, stage
        ));
        runlog::stage(&format!("register_shape id={} shape_timeout", id), 0);
        self.caches.shapes.lock().unwrap().insert_bounds_failed(key, bounds);

        let mut s = self.shared.lock().unwrap();
        s.diagnostics.shapes_registered = s.diagnostics.shapes_registered.saturating_add(1);
        s.diagnostics.total_tess_ms_fills = s.diagnostics.total_tess_ms_fills.saturating_add(elapsed_ms);
        s.diagnostics.total_tess_ms_strokes = s.diagnostics.total_tess_ms_strokes.saturating_add(0);
        s.diagnostics.max_tess_ms_single_shape = s.diagnostics.max_tess_ms_single_shape.max(elapsed_ms);
        shape_handle_from_impl(Arc::clone(handle_impl))
    }


    pub fn is_ready(&self) -> bool {
        let s = self.shared.lock().unwrap();
        s.diagnostics.movie_loaded && (s.diagnostics.frames_submitted > 0 || s.diagnostics.shapes_registered > 0)
    }

    pub fn has_seen_real_draw(&self) -> bool {
        let s = self.shared.lock().unwrap();
        s.seen_real_draw
    }

    pub fn status_text_short(&self) -> String {
        let s = self.shared.lock().unwrap();
        if let Some(fatal) = &s.diagnostics.last_fatal {
            // Keep it within 32 chars; C HUD prepends "FPS:xx ".
            return format!("ERR: {}", trim_to(fatal, 27));
        }

        // Keep this short: the C HUD prepends "FPS:xx".
        let mode = if s.seen_real_draw { "OK" } else { "LD" };
        let mut line = format!(
            "{} v{} t:{} sh:{} S:{} B:{}",
            mode,
            s.diagnostics.swf_version,
            s.diagnostics.last_tris,
            s.diagnostics.shapes_registered,
            s.diagnostics.last_cmds_shapes,
            s.diagnostics.last_cmds_bitmaps,
        );
        if let Some(input) = &s.diagnostics.last_input {
            line = format!("{} I:{}", line, trim_to(input, 9));
        }
        if let Some(warn) = &s.diagnostics.last_warning {
            // Prefix warnings so the C HUD can show them on a dedicated line above the main HUD.
            line = format!("!{} {}", trim_to(warn, 10), line);
        }
        // C HUD prepends "FPS:xx " (7 chars), and the bottom console line is 40 chars.
        trim_to(&line, 32).to_string()
    }

    pub fn status_snapshot_full(&self) -> String {
        struct SnapshotDiag {
            seen_real_draw: bool,
            swf_version: u8,
            shapes_registered: u32,
            bitmaps_registered: u32,
            frames_submitted: u32,
            last_cmds_total: u32,
            last_cmds_shapes: u32,
            last_cmds_bitmaps: u32,
            last_cmds_other: u32,
            last_tris: u32,
            total_tess_ms_fills: u64,
            total_tess_ms_strokes: u64,
            max_tess_ms_single_shape: u64,
            total_group_more_correct: u32,
            total_group_fast: u32,
            total_group_trivial: u32,
            total_unsupported_fill_paints: u32,
            last_warning: Option<String>,
            last_fatal: Option<String>,
        }

        let diag = {
            let s = self.shared.lock().unwrap();
            SnapshotDiag {
                seen_real_draw: s.seen_real_draw,
                swf_version: s.diagnostics.swf_version,
                shapes_registered: s.diagnostics.shapes_registered,
                bitmaps_registered: s.diagnostics.bitmaps_registered,
                frames_submitted: s.diagnostics.frames_submitted,
                last_cmds_total: s.diagnostics.last_cmds_total,
                last_cmds_shapes: s.diagnostics.last_cmds_shapes,
                last_cmds_bitmaps: s.diagnostics.last_cmds_bitmaps,
                last_cmds_other: s.diagnostics.last_cmds_other,
                last_tris: s.diagnostics.last_tris,
                total_tess_ms_fills: s.diagnostics.total_tess_ms_fills,
                total_tess_ms_strokes: s.diagnostics.total_tess_ms_strokes,
                max_tess_ms_single_shape: s.diagnostics.max_tess_ms_single_shape,
                total_group_more_correct: s.diagnostics.total_group_more_correct,
                total_group_fast: s.diagnostics.total_group_fast,
                total_group_trivial: s.diagnostics.total_group_trivial,
                total_unsupported_fill_paints: s.diagnostics.total_unsupported_fill_paints,
                last_warning: s.diagnostics.last_warning.clone(),
                last_fatal: s.diagnostics.last_fatal.clone(),
            }
        };

        let shapes_cache = self.caches.shapes.lock().unwrap();
        let (fill_missing, fill_invalid, fill_bounds) = shapes_cache.stats();
        let (stroke_missing, stroke_invalid, stroke_bounds) = shapes_cache.stroke_stats();
        let (cache_used_bytes, cache_budget_bytes, cache_evicted_entries, cache_evicted_bytes) = shapes_cache.mem_stats();
        let draw_stats = crate::render::executor::last_draw_stats();
        let runlog_info = runlog::snapshot_info();

        let mut out = String::new();
        let mode = if diag.seen_real_draw { "OK" } else { "LD" };
        out.push_str(&format!(
            "mode={} swf_v={} frames_submitted={}\n",
            mode, diag.swf_version, diag.frames_submitted
        ));
        out.push_str(&format!(
            "registered shapes={} bitmaps={}\n",
            diag.shapes_registered, diag.bitmaps_registered
        ));
        out.push_str(&format!(
            "last_frame cmds total={} shapes={} bitmaps={} other={} tris={}\n",
            diag.last_cmds_total,
            diag.last_cmds_shapes,
            diag.last_cmds_bitmaps,
            diag.last_cmds_other,
            diag.last_tris
        ));
        out.push_str(&format!(
            "shape_tess_timing totals_fills_ms={} totals_strokes_ms={} max_shape_ms={}\n",
            diag.total_tess_ms_fills,
            diag.total_tess_ms_strokes,
            diag.max_tess_ms_single_shape
        ));
        out.push_str(&format!(
            "shape_grouping totals more_correct={} fast={} trivial={} unsupported_fills={}\n",
            diag.total_group_more_correct,
            diag.total_group_fast,
            diag.total_group_trivial,
            diag.total_unsupported_fill_paints
        ));
        out.push_str(&format!(
            "shape_cache fill missing={} invalid={} bounds_fallbacks={} stroke missing={} invalid={} bounds_fallbacks={}\n",
            fill_missing,
            fill_invalid,
            fill_bounds,
            stroke_missing,
            stroke_invalid,
            stroke_bounds
        ));
        out.push_str(&format!(
            "shape_cache_mem used_kb={} budget_kb={} evicted_entries={} evicted_kb={}\n",
            cache_used_bytes / 1024,
            cache_budget_bytes / 1024,
            cache_evicted_entries,
            cache_evicted_bytes / 1024
        ));
        out.push_str(&format!(
            "draw_stats mesh_tris={} rect_fastpath={} bounds_fallbacks={}\n",
            draw_stats.mesh_tris,
            draw_stats.rect_fastpath,
            draw_stats.bounds_fallbacks
        ));

        if let Some(info) = runlog_info {
            out.push_str(&format!(
                "last_stage frame={} stage={}\n",
                info.last_stage_frame, info.last_stage
            ));
            if !info.recent_warnings.is_empty() {
                out.push_str("recent_warnings:\n");
                for warning in info.recent_warnings {
                    out.push_str(&format!("  - {}\n", warning));
                }
            }
        }

        if let Some(warn) = diag.last_warning {
            out.push_str(&format!("last_warning={}\n", warn));
        }
        if let Some(fatal) = diag.last_fatal {
            out.push_str(&format!("last_fatal={}\n", fatal));
        }

        out
    }
}

// --------------------------
// Render backend (Ruffle → internal FramePacket)
// --------------------------

#[derive(Debug)]
pub struct ThreeDSShapeHandleImpl {
    #[allow(dead_code)]
    pub id: u32,
}

impl ShapeHandleImpl for ThreeDSShapeHandleImpl {}

#[derive(Debug)]
pub struct ThreeDSBitmapHandleImpl {
    #[allow(dead_code)]
    pub id: u32,
}

impl BitmapHandleImpl for ThreeDSBitmapHandleImpl {}

impl RenderBackend for ThreeDSBackend {
    fn viewport_dimensions(&self) -> ViewportDimensions {
        ViewportDimensions { width: 400, height: 240, scale_factor: 1.0 }
    }

    fn set_viewport_dimensions(&mut self, _dimensions: ViewportDimensions) {}

    /// Register and tessellate a shape at load time.
    ///
    /// Fail-fast safety: if tessellation/earcut work for this shape exceeds 15ms wall-clock,
    /// registration aborts immediately, logs a "Shape Timeout", and falls back to bounds-only rendering.
    fn register_shape(&mut self, shape: DistilledShape<'_>, _bitmap: &dyn BitmapSource) -> ShapeHandle {
        // Timing logs capture tessellation hotspots per shape so we can correlate slow meshes
        // with shape IDs/bounds without altering the render path.
        let id = self.next_shape_id.fetch_add(1, Ordering::Relaxed);
        let handle_impl = Arc::new(ThreeDSShapeHandleImpl { id });
        let key: ShapeKey = Arc::as_ptr(&handle_impl) as *const () as ShapeKey;

        // Compute bounds in pixel units.
        let b = shape.shape_bounds;
        let x0 = b.x_min.to_pixels() as i32;
        let y0 = b.y_min.to_pixels() as i32;
        let x1 = b.x_max.to_pixels() as i32;
        let y1 = b.y_max.to_pixels() as i32;
        let bounds = RectI { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };

        let text_shape = is_text_shape(&shape);
        if text_shape {
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
            let fills = vec![FillMesh { verts, indices, paint: FillPaint::Unsupported }];
            self.caches.shapes.lock().unwrap().insert_meshes(
                key,
                id,
                bounds,
                fills,
                false,
                false,
                Vec::new(),
                false,
                false,
                true,
            );

            let mut s = self.shared.lock().unwrap();
            s.diagnostics.shapes_registered = s.diagnostics.shapes_registered.saturating_add(1);
            shape_handle_from_impl(handle_impl)
        } else {
            // Step 2A: tessellate fills once at registration time and cache the meshes.
            //
            // Shapes commonly contain multiple fills, so we cache one mesh per fill. If some fills
            // fail (hard cases), we still keep the successful ones and mark `tess_failed` so the HUD
            // can warn when that shape is drawn.
            runlog::stage(&format!("register_shape id={} pre_tess", id), 0);
            if runlog::is_verbose() {
                runlog::log_line(&format!("register_shape begin id={} b={} {} {} {}", id, bounds.x, bounds.y, bounds.w, bounds.h));
            }

            let shape_start = Instant::now();

            let fills_start = Instant::now();
            let (fills, fill_failed, fill_partial, group_used_more_correct, group_used_fast, group_used_trivial, unsupported_fill_paints) =
                match tessellate::tessellate_fills(&shape, id) {
                Ok(res) => (
                    res.fills,
                    false,
                    res.any_failed,
                    res.group_used_more_correct,
                    res.group_used_fast,
                    res.group_used_trivial,
                    res.unsupported_fill_paints,
                ),
                Err(tessellate::TessError::NoContours) => (Vec::new(), false, false, 0, 0, 0, 0),
                Err(tessellate::TessError::Timeout) => {
                    runlog::stage(&format!("register_shape id={} tess_timeout", id), 0);
                    (Vec::new(), true, false, 0, 0, 0, 0)
                }
                Err(tessellate::TessError::EarcutDenied) => (Vec::new(), true, false, 0, 0, 0, 0),
                Err(_) => (Vec::new(), true, false, 0, 0, 0, 0),
                };
            let fills_ms = fills_start.elapsed().as_millis() as u64;
            let elapsed_ms = shape_start.elapsed().as_millis() as u64;
            if elapsed_ms > SHAPE_WATCHDOG_MS {
                return self.shape_timeout_fallback(key, id, bounds, elapsed_ms, "post_fills", &handle_impl);
            }
            let skip_strokes = !shape.paths.iter().any(|path| matches!(path, DrawPath::Stroke { .. }));
            let (strokes, stroke_failed, stroke_partial, strokes_ms) = if skip_strokes {
                (Vec::new(), false, false, 0)
            } else {
                let elapsed_ms = shape_start.elapsed().as_millis() as u64;
                if elapsed_ms > SHAPE_WATCHDOG_MS {
                    return self.shape_timeout_fallback(key, id, bounds, elapsed_ms, "pre_strokes", &handle_impl);
                }
                let strokes_start = Instant::now();
                let (strokes, stroke_failed, stroke_partial) = match tessellate::tessellate_strokes(&shape, id) {
                    Ok(res) => (res.strokes, false, res.any_failed),
                    Err(tessellate::TessError::NoContours) => (Vec::new(), false, false),
                    Err(_) => (Vec::new(), true, false),
                };
                let strokes_ms = strokes_start.elapsed().as_millis() as u64;
                (strokes, stroke_failed, stroke_partial, strokes_ms)
            };
            let elapsed_ms = shape_start.elapsed().as_millis() as u64;
            if elapsed_ms > SHAPE_WATCHDOG_MS {
                return self.shape_timeout_fallback(key, id, bounds, elapsed_ms, "post_strokes", &handle_impl);
            }

            let (fill_count, stroke_count, fill_tris, stroke_tris) = if runlog::is_verbose() {
                let fill_tris: u32 = fills.iter().map(|mesh| (mesh.indices.len() as u32) / 3).sum::<u32>();
                let stroke_tris: u32 = if skip_strokes {
                    0
                } else {
                    strokes.iter().map(|mesh| (mesh.indices.len() as u32) / 3).sum::<u32>()
                };
                let fill_count: Option<u32> = Some(fills.len() as u32);
                let stroke_count: Option<u32> = Some(strokes.len() as u32);
                let fill_tris: Option<u32> = Some(fill_tris);
                let stroke_tris: Option<u32> = Some(stroke_tris);
                (fill_count, stroke_count, fill_tris, stroke_tris)
            } else {
                let fill_count: Option<u32> = None;
                let stroke_count: Option<u32> = None;
                let fill_tris: Option<u32> = None;
                let stroke_tris: Option<u32> = None;
                (fill_count, stroke_count, fill_tris, stroke_tris)
            };

            self.caches.shapes.lock().unwrap().insert_meshes(
                key,
                id,
                bounds,
                fills,
                fill_failed,
                fill_partial,
                strokes,
                stroke_failed,
                stroke_partial,
                text_shape,
            );

            if runlog::is_verbose() {
                runlog::log_line(&format!(
                    "shape_summary id={} b={} {} {} {} fills={} fill_tris={} strokes={} stroke_tris={} tess_failed={} tess_partial={} stroke_failed={} stroke_partial={} text={} group_more_correct={} group_fast={} group_trivial={} unsupported_fills={}",
                    id,
                    bounds.x,
                    bounds.y,
                    bounds.w,
                    bounds.h,
                    fill_count.unwrap_or(0),
                    fill_tris.unwrap_or(0),
                    stroke_count.unwrap_or(0),
                    stroke_tris.unwrap_or(0),
                    fill_failed,
                    fill_partial,
                    stroke_failed,
                    stroke_partial,
                    text_shape,
                    group_used_more_correct,
                    group_used_fast,
                    group_used_trivial,
                    unsupported_fill_paints
                ));
                runlog::log_line(&format!(
                    "shape_register_timing summary id={} b={} {} {} {} fills_ms={} strokes_ms={} fills={} fill_tris={} strokes={} stroke_tris={}",
                    id,
                    bounds.x,
                    bounds.y,
                    bounds.w,
                    bounds.h,
                    fills_ms,
                    strokes_ms,
                    fill_count.unwrap_or(0),
                    fill_tris.unwrap_or(0),
                    stroke_count.unwrap_or(0),
                    stroke_tris.unwrap_or(0)
                ));
            }

            if fill_failed {
                if runlog::is_verbose() {
                    runlog::warn_line(&format!("tessellate_fills fallback_bounds id={}", id));
                } else if (id % 25) == 0 {
                    runlog::log_important(&format!("tessellate_fills fallback_bounds id={} (sampled)", id));
                }
                runlog::stage(&format!("register_shape id={} fill_fallback_bounds", id), 0);
            } else if runlog::is_verbose() {
                runlog::log_line(&format!("tessellate_fills ok id={} any_failed={}", id, fill_partial));
            }

            if stroke_failed && runlog::is_verbose() {
                runlog::warn_line(&format!("tessellate_strokes fallback_bounds id={}", id));
            } else if stroke_partial && runlog::is_verbose() {
                runlog::log_line(&format!("tessellate_strokes partial id={}", id));
            }

            runlog::stage(&format!("register_shape id={} done", id), 0);

            let mut s = self.shared.lock().unwrap();
            s.diagnostics.shapes_registered = s.diagnostics.shapes_registered.saturating_add(1);
            s.diagnostics.total_tess_ms_fills = s.diagnostics.total_tess_ms_fills.saturating_add(fills_ms);
            s.diagnostics.total_tess_ms_strokes = s.diagnostics.total_tess_ms_strokes.saturating_add(strokes_ms);
            let shape_total_ms = fills_ms.saturating_add(strokes_ms);
            s.diagnostics.max_tess_ms_single_shape = s.diagnostics.max_tess_ms_single_shape.max(shape_total_ms);
            s.diagnostics.total_group_more_correct = s.diagnostics.total_group_more_correct.saturating_add(group_used_more_correct);
            s.diagnostics.total_group_fast = s.diagnostics.total_group_fast.saturating_add(group_used_fast);
            s.diagnostics.total_group_trivial = s.diagnostics.total_group_trivial.saturating_add(group_used_trivial);
            s.diagnostics.total_unsupported_fill_paints = s
                .diagnostics
                .total_unsupported_fill_paints
                .saturating_add(unsupported_fill_paints);
            shape_handle_from_impl(handle_impl)
        }
    }

    fn submit_frame(&mut self, clear: Color, commands: CommandList, _cache: Vec<BitmapCacheEntry>) {
        let mut s = self.shared.lock().unwrap();
        s.frame.reset(clear);
        s.diagnostics.last_tris = 0;

        let wire_once = s.wireframe_once || s.wireframe_hold;
        // Wireframe is a one-shot flag.
        s.wireframe_once = false;

        let shapes_cache = self.caches.shapes.lock().unwrap();

        let mut total: u32 = 0;
        let mut shapes: u32 = 0;
        let mut bitmaps: u32 = 0;
        let mut other: u32 = 0;
        let mut tris_budget = MAX_TRIS_PER_FRAME;
        let mut tri_cap_warned = false;

        if s.dump_next_frame {
            println!("[3DS] submit_frame: {} commands", commands.commands.len());
        }

        let mut mask_pending_rect: Option<RectI> = None;
        let mut mask_mode = false;

        for (i, cmd) in commands.commands.iter().enumerate() {
            total = total.saturating_add(1);
            match cmd {
                Command::PushMask => {
                    mask_mode = true;
                    mask_pending_rect = None;
                    other = other.saturating_add(1);
                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: PushMask");
                    }
                }
                Command::ActivateMask => {
                    if let Some(rect) = mask_pending_rect.take() {
                        s.frame.cmds.push(RenderCmd::PushMaskRect { rect });
                    } else {
                        runlog::warn_line("mask activate without rect; ignoring");
                    }
                    mask_mode = false;
                    other = other.saturating_add(1);
                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: ActivateMask");
                    }
                }
                Command::DeactivateMask => {
                    mask_mode = false;
                    other = other.saturating_add(1);
                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: DeactivateMask");
                    }
                }
                Command::PopMask => {
                    s.frame.cmds.push(RenderCmd::PopMask);
                    other = other.saturating_add(1);
                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: PopMask");
                    }
                }
                Command::DrawRect { matrix, .. } => {
                    if mask_mode {
                        let axis_aligned = matrix.b == 0.0 && matrix.c == 0.0;
                        if axis_aligned {
                            // DrawRect uses a unit rect; scale by 1.0 then apply translation.
                            let x = matrix.tx.to_pixels() as i32;
                            let y = matrix.ty.to_pixels() as i32;
                            let w = matrix.a.abs().round() as i32;
                            let h = matrix.d.abs().round() as i32;
                            if w > 0 && h > 0 {
                                mask_pending_rect = Some(RectI { x, y, w, h });
                            } else {
                                runlog::warn_line("mask rect has zero size; ignoring");
                            }
                        } else {
                            runlog::warn_line("non-axis-aligned mask rect unsupported; ignoring");
                        }
                        other = other.saturating_add(1);
                        if s.dump_next_frame && i < 32 {
                            println!("  {i}: DrawRect(mask)");
                        }
                    } else {
                        other = other.saturating_add(1);
                        if s.dump_next_frame && i < 32 {
                            println!("  {i}: DrawRect");
                        }
                    }
                }
                Command::RenderShape { shape, transform, .. } => {
                    shapes = shapes.saturating_add(1);
                    s.seen_real_draw = true;

                    let key: ShapeKey = Arc::as_ptr(&shape.0) as *const () as ShapeKey;
                    let matrix = Matrix2D {
                        a: transform.matrix.a,
                        b: transform.matrix.b,
                        c: transform.matrix.c,
                        d: transform.matrix.d,
                        tx: transform.matrix.tx.to_pixels() as f32,
                        ty: transform.matrix.ty.to_pixels() as f32,
                    };
                    let color_transform = to_color_transform(transform.color_transform);

                    if let Some(b) = shapes_cache.get_bounds(key) {
                        // Per-shape early reject using transformed bounds.
                        // This avoids pushing per-fill commands for offscreen sprites.
                        let tr = rect_aabb_transformed(b, matrix);
                        if tr.x + tr.w <= 0 || tr.y + tr.h <= 0 || tr.x >= 400 || tr.y >= 240 {
                            continue;
                        }

                        shapes_cache.touch(key);

                        let is_text = shapes_cache.is_text_shape(key);
                        if shapes_cache.has_mesh(key) {
                            let shape_tris = shapes_cache.get_total_tri_count(key);
                            if shape_tris > tris_budget {
                                s.frame.cmds.push(RenderCmd::FillRect { rect: tr, color_key: key as u64, wireframe: wire_once });
                                if s.diagnostics.last_warning.is_none() {
                                    s.diagnostics.last_warning = Some("tri_cap".to_string());
                                }
                                if !tri_cap_warned {
                                    runlog::warn_line("tri_cap budget exceeded; falling back to bounds");
                                    tri_cap_warned = true;
                                }
                                continue;
                            }
                            let fill_count = shapes_cache.fill_count(key);
                            if fill_count == 0 {
                                if is_text {
                                    s.frame.cmds.push(RenderCmd::DrawTextSolidFill {
                                        shape_key: key,
                                        fill_idx: 0,
                                        transform: matrix,
                                        solid_rgba: None,
                                        color_transform,
                                        color_key: key as u64,
                                        wireframe: wire_once,
                                    });
                                } else {
                                    s.frame.cmds.push(RenderCmd::FillRect { rect: tr, color_key: key as u64, wireframe: wire_once });
                                }
                                if s.diagnostics.last_warning.is_none() {
                                    s.diagnostics.last_warning = Some("tri_miss".to_string());
                                }
                                runlog::warn_line(&format!("shape_fill_missing key={}", key));
                                continue;
                            }
                            // Emit one draw cmd per fill mesh.
                            for fi in 0..fill_count {
                                let color_key = (key as u64) ^ ((fi as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                                let solid_rgba = shapes_cache
                                    .get_fill_mesh(key, fi)
                                    .map(|mesh| match mesh.paint {
                                        crate::render::cache::shapes::FillPaint::SolidRGBA(r, g, b, a) => Some([r, g, b, a]),
                                        crate::render::cache::shapes::FillPaint::Unsupported => None,
                                    })
                                    .unwrap_or(None);
                                if solid_rgba.is_none() {
                                    let warn_count = UNSUPPORTED_FILL_DRAW_WARNINGS.fetch_add(1, Ordering::Relaxed);
                                    if warn_count < MAX_UNSUPPORTED_FILL_WARNINGS {
                                        runlog::warn_line(&format!(
                                            "shape_fill_unsupported shape={} fill={}",
                                            key, fi
                                        ));
                                    }
                                }
                                if is_text {
                                    s.frame.cmds.push(RenderCmd::DrawTextSolidFill {
                                        shape_key: key,
                                        fill_idx: fi as u16,
                                        transform: matrix,
                                        solid_rgba,
                                        color_transform,
                                        color_key,
                                        wireframe: wire_once,
                                    });
                                } else {
                                    s.frame.cmds.push(RenderCmd::DrawShapeSolidFill {
                                        shape_key: key,
                                        fill_idx: fi as u16,
                                        transform: matrix,
                                        solid_rgba,
                                        color_transform,
                                        color_key,
                                        wireframe: wire_once,
                                    });
                                }
                            }
                            s.diagnostics.last_tris = s.diagnostics.last_tris.saturating_add(
                                shapes_cache.get_total_tri_count(key),
                            );
                            tris_budget = tris_budget.saturating_sub(shape_tris);

                            if shapes_cache.is_tess_partial(key) && s.diagnostics.last_warning.is_none() {
                                s.diagnostics.last_warning = Some("tri_part".to_string());
                            }
                        } else {
                            // Fallback: bounds rect.
                            s.frame.cmds.push(RenderCmd::FillRect { rect: tr, color_key: key as u64, wireframe: wire_once });
                            if s.diagnostics.last_warning.is_none() {
                                let warn = if shapes_cache.is_tess_failed(key) {
                                    "tri_fail"
                                } else {
                                    "tri_miss"
                                };
                                s.diagnostics.last_warning = Some(warn.to_string());
                            }
                        }

                        let stroke_count = shapes_cache.stroke_count(key);
                        if stroke_count > 0 {
                            for si in 0..stroke_count {
                                let color = shapes_cache
                                    .get_stroke_mesh(key, si)
                                    .map(|s| (s.r, s.g, s.b))
                                    .unwrap_or((255, 255, 255));
                                s.frame.cmds.push(RenderCmd::DrawShapeStroke {
                                    shape_key: key,
                                    stroke_idx: si as u16,
                                    transform: matrix,
                                    r: color.0,
                                    g: color.1,
                                    b: color.2,
                                    wireframe: wire_once,
                                });
                            }
                            if shapes_cache.is_stroke_partial(key) && s.diagnostics.last_warning.is_none() {
                                s.diagnostics.last_warning = Some("str_part".to_string());
                            }
                        } else if shapes_cache.is_stroke_failed(key) {
                            let color_key = (key as u64) ^ 0xA5A5_5A5A_F0F0_0F0F;
                            let (r, g, b) = debug_color_from_key(color_key);
                            s.frame.cmds.push(RenderCmd::DrawShapeStroke {
                                shape_key: key,
                                stroke_idx: 0,
                                transform: matrix,
                                r,
                                g,
                                b,
                                wireframe: wire_once,
                            });
                            if s.diagnostics.last_warning.is_none() {
                                s.diagnostics.last_warning = Some("str_fail".to_string());
                            }
                        }
                    } else if s.diagnostics.last_warning.is_none() {
                        s.diagnostics.last_warning = Some("miss_shp".to_string());
                    }

                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: RenderShape");
                    }
                }
                Command::RenderBitmap { bitmap, transform, .. } => {
                    bitmaps = bitmaps.saturating_add(1);
                    s.seen_real_draw = true;

                    let key = Arc::as_ptr(&bitmap.0) as *const () as usize;
                    let tx = transform.matrix.tx.to_pixels() as f32;
                    let ty = transform.matrix.ty.to_pixels() as f32;
                    let matrix = Matrix2D {
                        a: transform.matrix.a,
                        b: transform.matrix.b,
                        c: transform.matrix.c,
                        d: transform.matrix.d,
                        tx,
                        ty,
                    };
                    let color_transform = to_color_transform(transform.color_transform);

                    // Only push a blit if the bitmap exists; otherwise keep a short warning.
                    if self.caches.bitmaps.lock().unwrap().contains_key(key) {
                        s.frame.cmds.push(RenderCmd::BlitBitmap {
                            bitmap_key: key,
                            transform: matrix,
                            uv: TexUvRect::full(),
                            color_transform,
                        });
                    } else if s.diagnostics.last_warning.is_none() {
                        s.diagnostics.last_warning = Some("miss_bmp".to_string());
                    }

                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: RenderBitmap");
                    }
                }
                _ => {
                    other = other.saturating_add(1);
                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: Other");
                    }
                }
            }
        }

        if s.dump_next_frame {
            s.dump_next_frame = false;
            println!("[3DS] totals: cmds={total} shapes={shapes} bitmaps={bitmaps} other={other}");
        }

        s.diagnostics.frames_submitted = s.diagnostics.frames_submitted.saturating_add(1);
        s.diagnostics.last_cmds_total = total;
        s.diagnostics.last_cmds_shapes = shapes;
        s.diagnostics.last_cmds_bitmaps = bitmaps;
        s.diagnostics.last_cmds_other = other;
        s.submit_called = true;
    }

    fn render_offscreen(
        &mut self,
        _handle: BitmapHandle,
        _commands: CommandList,
        _quality: StageQuality,
        _region: PixelRegion,
    ) -> Option<Box<dyn SyncHandle>> {
        None
    }

    fn create_empty_texture(&mut self, width: u32, height: u32) -> Result<BitmapHandle, RenderError> {
        let id = self.next_bitmap_id.fetch_add(1, Ordering::Relaxed);
        let handle_impl = Arc::new(ThreeDSBitmapHandleImpl { id });
        let key = Arc::as_ptr(&handle_impl) as *const () as usize;

        let surface = BitmapSurface {
            width,
            height,
            rgba: vec![0u8; (width as usize) * (height as usize) * 4],
            is_opaque: false,
        };
        self.caches.bitmaps.lock().unwrap().insert(key, surface);

        let mut s = self.shared.lock().unwrap();
        s.diagnostics.bitmaps_registered = s.diagnostics.bitmaps_registered.saturating_add(1);
        Ok(BitmapHandle(handle_impl))
    }

    fn register_bitmap(&mut self, bitmap: Bitmap) -> Result<BitmapHandle, RenderError> {
        let id = self.next_bitmap_id.fetch_add(1, Ordering::Relaxed);
        let handle_impl = Arc::new(ThreeDSBitmapHandleImpl { id });
        let key = Arc::as_ptr(&handle_impl) as *const () as usize;

        let surface = bitmap_to_surface(bitmap);
        self.caches.bitmaps.lock().unwrap().insert(key, surface);

        let mut s = self.shared.lock().unwrap();
        s.diagnostics.bitmaps_registered = s.diagnostics.bitmaps_registered.saturating_add(1);
        Ok(BitmapHandle(handle_impl))
    }

    fn update_texture(&mut self, handle: &BitmapHandle, bitmap: Bitmap, _region: PixelRegion) -> Result<(), RenderError> {
        // Step 3 bootstrap: we ignore partial region updates and replace the full surface.
        let key = Arc::as_ptr(&handle.0) as *const () as usize;
        let surface = bitmap_to_surface(bitmap);
        self.caches.bitmaps.lock().unwrap().insert(key, surface);
        Ok(())
    }

    fn create_context3d(&mut self, _profile: Context3DProfile) -> Result<Box<dyn Context3D>, RenderError> {
        Err(RenderError::Unimplemented("Context3D".into()))
    }

    fn debug_info(&self) -> Cow<'static, str> {
        Cow::Borrowed("3DS")
    }

    fn name(&self) -> &'static str {
        "3DS"
    }

    fn set_quality(&mut self, _quality: StageQuality) {}

    fn compile_pixelbender_shader(&mut self, _shader: PixelBenderShader) -> Result<PixelBenderShaderHandle, RenderError> {
        Err(RenderError::Unimplemented("PixelBender".into()))
    }

    fn run_pixelbender_shader(
        &mut self,
        _handle: PixelBenderShaderHandle,
        _args: &[PixelBenderShaderArgument],
        _target: &PixelBenderTarget,
    ) -> Result<PixelBenderOutput, RenderError> {
        Err(RenderError::Unimplemented("PixelBender".into()))
    }

    fn resolve_sync_handle(&mut self, _handle: Box<dyn SyncHandle>, _callback: RgbaBufRead) -> Result<(), RenderError> {
        Ok(())
    }
}


#[cfg(feature = "net")]
impl NavigatorBackend for ThreeDSBackend {
    fn navigate_to_url(&self, _url: &str, _target: &str, _vars: Option<(NavigationMethod, IndexMap<String, String>)>) {}

    fn fetch(&self, _request: Request) -> Pin<Box<dyn Future<Output = Result<Box<dyn SuccessResponse>, ErrorResponse>>>> {
        Box::pin(async move {
            Err(ErrorResponse {
                url: "".to_string(),
                error: std::io::Error::new(std::io::ErrorKind::NotFound, "Navigator fetch unimplemented").into(),
            })
        })
    }

    fn resolve_url(&self, url: &str) -> Result<Url, url::ParseError> {
        Url::parse(url)
    }

    fn spawn_future(&mut self, future: Pin<Box<dyn Future<Output = Result<(), DialogLoaderError>>>>) {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.push(Box::pin(async move {
            let _ = future.await;
        }));
    }

    fn pre_process_url(&self, url: Url) -> Url { url }

    fn connect_socket(&mut self, _host: String, _port: u16, _timeout: Duration, _handle: SocketHandle, _receiver: Receiver<Vec<u8>>, _sender: Sender<SocketAction>) {
        runlog::warn_line("navigator connect_socket unimplemented");
    }
}

#[cfg(feature = "storage")]
impl StorageBackend for ThreeDSBackend {
    fn get(&self, _key: &str) -> Option<Vec<u8>> { None }
    fn put(&mut self, _key: &str, _value: &[u8]) -> bool { false }
    fn remove_key(&mut self, _key: &str) {}
}

impl UiBackend for ThreeDSBackend {
    fn mouse_visible(&self) -> bool { true }
    fn set_mouse_visible(&mut self, _visible: bool) {}
    fn set_mouse_cursor(&mut self, _cursor: MouseCursor) {}

    fn clipboard_content(&mut self) -> String { String::new() }
    fn set_clipboard_content(&mut self, _content: String) {}

    fn set_fullscreen(&mut self, _is_full: bool) -> Result<(), Cow<'static, str>> { Ok(()) }
    fn display_root_movie_download_failed_message(&self, _unknown: bool, _msg: String) {}
    fn message(&self, _message: &str) {}
    fn open_virtual_keyboard(&self) {}
    fn close_virtual_keyboard(&self) {}

    fn language(&self) -> LanguageIdentifier { "en-US".parse().unwrap() }
    fn display_unsupported_video(&self, _url: Url) {}

    fn load_device_font(&self, _query: &FontQuery, _callback: &mut dyn FnMut(FontDefinition)) {}
    fn sort_device_fonts(&self, _query: &FontQuery, _callback: &mut dyn FnMut(FontDefinition)) -> Vec<FontQuery> { vec![] }

    fn display_file_open_dialog(&mut self, _filter: Vec<FileFilter>) -> Option<Pin<Box<dyn Future<Output = Result<Box<dyn FileDialogResult>, DialogLoaderError>>>>> { None }
    fn display_file_save_dialog(&mut self, _title: String, _filter: String) -> Option<Pin<Box<dyn Future<Output = Result<Box<dyn FileDialogResult>, DialogLoaderError>>>>> { None }
    fn close_file_dialog(&mut self) {}
}

impl LogBackend for ThreeDSBackend {
    fn avm_trace(&self, message: &str) { println!("[AVM] {}", message); }
    fn avm_warning(&self, message: &str) { println!("[AVM Warn] {}", message); }
}

// --------------------------
// Small helpers
// --------------------------

fn trim_to(s: &str, n: usize) -> &str {
    if s.len() <= n { return s; }
    &s[..n]
}

#[cfg(feature = "net")]
unsafe fn dummy_waker_clone(_: *const ()) -> RawWaker { dummy_waker() }
#[cfg(feature = "net")]
unsafe fn dummy_waker_wake(_: *const ()) {}
#[cfg(feature = "net")]
unsafe fn dummy_waker_wake_by_ref(_: *const ()) {}
#[cfg(feature = "net")]
unsafe fn dummy_waker_drop(_: *const ()) {}

#[cfg(feature = "net")]
const VTABLE: RawWakerVTable = RawWakerVTable::new(
    dummy_waker_clone,
    dummy_waker_wake,
    dummy_waker_wake_by_ref,
    dummy_waker_drop,
);

#[cfg(feature = "net")]
fn dummy_waker() -> RawWaker {
    RawWaker::new(std::ptr::null(), &VTABLE)
}

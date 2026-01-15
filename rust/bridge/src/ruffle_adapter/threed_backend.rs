use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use indexmap::IndexMap;
use url::Url;

use ruffle_core::backend::log::LogBackend;
use ruffle_core::backend::navigator::{NavigatorBackend, NavigationMethod, Request, SuccessResponse, ErrorResponse};
use ruffle_core::backend::storage::StorageBackend;
use ruffle_core::backend::ui::{UiBackend, MouseCursor, FileFilter, FileDialogResult, FontDefinition, LanguageIdentifier, DialogLoaderError};
use ruffle_core::font::FontQuery;
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
use ruffle_render::shape_utils::DistilledShape;
use ruffle_render::pixel_bender::{PixelBenderShader, PixelBenderShaderHandle};
use ruffle_render::pixel_bender_support::PixelBenderShaderArgument;

use crate::render::{FramePacket, RenderCmd, RectI, SharedCaches};
use crate::render::cache::bitmaps::BitmapSurface;

// Step 2A tessellator lives next to this backend inside ruffle_adapter/.
use super::tessellate;
use crate::runlog;
type ShapeKey = usize;

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
    BitmapSurface { width, height, rgba }
}

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
    last_warning: Option<String>,
    last_fatal: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShapeMode {
    Rect,
    Tris,
}

struct SharedState {
    frame: FramePacket,
    submit_called: bool,
    seen_real_draw: bool,
    dump_next_frame: bool,
    diagnostics: Diagnostics,
    shape_mode: ShapeMode,
    wireframe_once: bool,
    wireframe_hold: bool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            frame: FramePacket::new(),
            submit_called: false,
            seen_real_draw: false,
            dump_next_frame: false,
            diagnostics: Diagnostics::default(),
            shape_mode: ShapeMode::Rect,
            wireframe_once: false,
            wireframe_hold: false,
        }
    }
}

#[derive(Clone)]
pub struct ThreeDSBackend {
    tasks: Arc<Mutex<Vec<BoxedFuture>>>,
    shared: Arc<Mutex<SharedState>>,
    next_shape_id: Arc<AtomicU32>,
    next_bitmap_id: Arc<AtomicU32>,
    caches: SharedCaches,
}

impl ThreeDSBackend {
    pub fn new(caches: SharedCaches) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(Vec::new())),
            shared: Arc::new(Mutex::new(SharedState::new())),
            next_shape_id: Arc::new(AtomicU32::new(1)),
            next_bitmap_id: Arc::new(AtomicU32::new(1)),
            caches,
        }
    }

    pub fn poll_tasks(&self) {
        let waker = unsafe { Waker::from_raw(dummy_waker()) };
        let mut cx = Context::from_waker(&waker);

        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain_mut(|fut| fut.as_mut().poll(&mut cx) == Poll::Pending);
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

    pub fn toggle_shape_mode(&self) {
        let mut s = self.shared.lock().unwrap();
        s.shape_mode = match s.shape_mode {
            ShapeMode::Rect => ShapeMode::Tris,
            ShapeMode::Tris => ShapeMode::Rect,
        };
    }

    pub fn toggle_wireframe_once(&self) {
        let mut s = self.shared.lock().unwrap();
        s.wireframe_once = true;
    }

    pub fn set_wireframe_hold(&self, enabled: bool) {
        let mut s = self.shared.lock().unwrap();
        s.wireframe_hold = enabled;
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
        let mflag = match s.shape_mode { ShapeMode::Tris => "mT", ShapeMode::Rect => "mR" };
        let mut line = format!(
            "{} v{} {} t:{} sh:{} S:{} B:{}",
            mode,
            s.diagnostics.swf_version,
            mflag,
            s.diagnostics.last_tris,
            s.diagnostics.shapes_registered,
            s.diagnostics.last_cmds_shapes,
            s.diagnostics.last_cmds_bitmaps,
        );
        if let Some(warn) = &s.diagnostics.last_warning {
            // Prefix warnings so the C HUD can show them on a dedicated line above the main HUD.
            line = format!("!{} {}", trim_to(warn, 10), line);
        }
        // C HUD prepends "FPS:xx " (7 chars), and the bottom console line is 40 chars.
        trim_to(&line, 32).to_string()
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

    fn register_shape(&mut self, shape: DistilledShape<'_>, _bitmap: &dyn BitmapSource) -> ShapeHandle {
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

        // Step 2A: tessellate fills once at registration time and cache the meshes.
        //
        // Shapes commonly contain multiple fills, so we cache one mesh per fill. If some fills
        // fail (hard cases), we still keep the successful ones and mark `tess_failed` so the HUD
        // can warn when that shape is drawn.
        runlog::stage(&format!("register_shape id={} pre_tess", id), 0);
        if runlog::is_verbose() {
            runlog::log_line(&format!("register_shape begin id={} b={} {} {} {}", id, bounds.x, bounds.y, bounds.w, bounds.h));
        }

        match tessellate::tessellate_fills(&shape) {
            Ok(res) if !res.fills.is_empty() => {
                self.caches
                    .shapes
                    .lock()
                    .unwrap()
                    .insert_fill_meshes(key, bounds, res.fills, res.any_failed);
                if runlog::is_verbose() {
                    runlog::log_line(&format!("tessellate_fills ok id={} any_failed={}", id, res.any_failed));
                }
                runlog::stage(&format!("register_shape id={} done", id), 0);
            }
            _ => {
                self.caches.shapes.lock().unwrap().insert_bounds_failed(key, bounds);
                // Important: some SWFs contain shapes we can't tessellate yet. Keep running.
                // Log lightly by default to preserve loading FPS.
                if runlog::is_verbose() {
                    runlog::warn_line(&format!("tessellate_fills fallback_bounds id={}", id));
                } else if (id % 25) == 0 {
                    runlog::log_important(&format!("tessellate_fills fallback_bounds id={} (sampled)", id));
                }
                runlog::stage(&format!("register_shape id={} fallback_bounds", id), 0);
            }
        }

        let mut s = self.shared.lock().unwrap();
        s.diagnostics.shapes_registered = s.diagnostics.shapes_registered.saturating_add(1);
        ShapeHandle(handle_impl)
    }

    fn submit_frame(&mut self, clear: Color, commands: CommandList, _cache: Vec<BitmapCacheEntry>) {
        let mut s = self.shared.lock().unwrap();
        s.frame.reset(clear);
        s.diagnostics.last_tris = 0;

        // Snapshot current mode flags for this frame.
        let mode_now = s.shape_mode;
        let wire_once = s.wireframe_once || s.wireframe_hold;
        // Wireframe is a one-shot flag.
        s.wireframe_once = false;

        let shapes_cache = self.caches.shapes.lock().unwrap();

        let mut total: u32 = 0;
        let mut shapes: u32 = 0;
        let mut bitmaps: u32 = 0;
        let mut other: u32 = 0;

        if s.dump_next_frame {
            println!("[3DS] submit_frame: {} commands", commands.commands.len());
        }

        for (i, cmd) in commands.commands.iter().enumerate() {
            total = total.saturating_add(1);
            match cmd {
                Command::RenderShape { shape, transform, .. } => {
                    shapes = shapes.saturating_add(1);
                    s.seen_real_draw = true;

                    let key: ShapeKey = Arc::as_ptr(&shape.0) as *const () as ShapeKey;
                    let tx = transform.matrix.tx.to_pixels() as i32;
                    let ty = transform.matrix.ty.to_pixels() as i32;

                    match mode_now {
                        ShapeMode::Tris => {
                            if let Some(b) = shapes_cache.get_bounds(key) {
                                // Per-shape early reject using translated bounds.
                                // This avoids pushing per-fill commands for offscreen sprites.
                                let tr = RectI { x: b.x + tx, y: b.y + ty, w: b.w, h: b.h };
                                if tr.x + tr.w <= 0 || tr.y + tr.h <= 0 || tr.x >= 400 || tr.y >= 240 {
                                    continue;
                                }

                                if shapes_cache.has_mesh(key) {
                                    let fill_count = shapes_cache.fill_count(key);
                                    // Emit one draw cmd per fill mesh.
                                    for fi in 0..fill_count {
                                        let color_key = (key as u64) ^ ((fi as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                                        s.frame.cmds.push(RenderCmd::DrawShapeSolidFill {
                                            shape_key: key,
                                            fill_idx: fi as u16,
                                            tx,
                                            ty,
                                            color_key,
                                            wireframe: wire_once,
                                        });
                                    }
                                    s.diagnostics.last_tris = s.diagnostics.last_tris.saturating_add(
                                        shapes_cache.get_total_tri_count(key),
                                    );

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
                            } else if s.diagnostics.last_warning.is_none() {
                                s.diagnostics.last_warning = Some("miss_shp".to_string());
                            }
                        }
                        ShapeMode::Rect => {
                            if let Some(b) = shapes_cache.get_bounds(key) {
                                let rect = RectI { x: b.x + tx, y: b.y + ty, w: b.w, h: b.h };
                                s.frame.cmds.push(RenderCmd::FillRect { rect, color_key: key as u64, wireframe: wire_once });
                            } else if s.diagnostics.last_warning.is_none() {
                                s.diagnostics.last_warning = Some("miss_shp".to_string());
                            }
                        }
                    }

                    if s.dump_next_frame && i < 32 {
                        println!("  {i}: RenderShape");
                    }
                }
                Command::RenderBitmap { bitmap, transform, .. } => {
                    bitmaps = bitmaps.saturating_add(1);
                    s.seen_real_draw = true;

                    let key = Arc::as_ptr(&bitmap.0) as *const () as usize;
                    let tx = transform.matrix.tx.to_pixels() as i32;
                    let ty = transform.matrix.ty.to_pixels() as i32;

                    // Only push a blit if the bitmap exists; otherwise keep a short warning.
                    if self.caches.bitmaps.lock().unwrap().contains_key(key) {
                        s.frame.cmds.push(RenderCmd::BlitBitmap { x: tx, y: ty, bitmap_key: key });
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

    fn connect_socket(&mut self, _host: String, _port: u16, _timeout: Duration, _handle: SocketHandle, _receiver: Receiver<Vec<u8>>, _sender: Sender<SocketAction>) {}
}

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

unsafe fn dummy_waker_clone(_: *const ()) -> RawWaker { dummy_waker() }
unsafe fn dummy_waker_wake(_: *const ()) {}
unsafe fn dummy_waker_wake_by_ref(_: *const ()) {}
unsafe fn dummy_waker_drop(_: *const ()) {}

const VTABLE: RawWakerVTable = RawWakerVTable::new(
    dummy_waker_clone,
    dummy_waker_wake,
    dummy_waker_wake_by_ref,
    dummy_waker_drop,
);

fn dummy_waker() -> RawWaker {
    RawWaker::new(std::ptr::null(), &VTABLE)
}
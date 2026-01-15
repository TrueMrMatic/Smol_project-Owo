use std::sync::{Arc, Mutex};

use ruffle_core::{Player, PlayerBuilder};
use ruffle_core::tag_utils::SwfMovie;
use ruffle_core::Color;

use ruffle_core::backend::audio::NullAudioBackend;
use ruffle_video::null::NullVideoBackend;

use crate::ffi::fileio::read_file_bytes;
use crate::ruffle_adapter::ThreeDSBackend;
use crate::render::{FramePacket, RenderCmd, Renderer, SharedCaches};
#[cfg(debug_assertions)]
use crate::render::Matrix2D;
use crate::runlog;

/// High-level engine state, owned by the C-side handle.
///
/// Design rule: C talks only to `Engine` through the FFI boundary.
pub struct Engine {
    player: Arc<Mutex<Player>>,
    backend: ThreeDSBackend,
    renderer: Renderer,
    scratch_packet: FramePacket,
    #[allow(dead_code)]
    root_path: String,
    #[allow(dead_code)]
    root_file_url: String,
    frame_counter: u64,
    last_heartbeat_ms: u64,
    pending_snapshot: Option<String>,
}

impl Engine {
    pub fn new(root_path_in: &str) -> Result<Self, String> {
        let root_path = root_path_in.to_string();
        let root_file_url = format!("file:///{}", root_path);

        runlog::init_for_swf(&root_path);
        runlog::log_important(&format!("Engine::new begin root_path={}", root_path));

        let movie_bytes = read_file_bytes(&root_path)
            .ok_or_else(|| format!("Could not read file: {}", root_path))?;
        runlog::log_important("Engine::new read_file ok");

        // Shared CPU-side caches (bitmaps now, shapes/mesh later).
        let caches = SharedCaches::new();

        // Backend shared between renderer/navigator/ui/log/storage.
        let backend = ThreeDSBackend::new(caches.clone());

        runlog::log_important("init: player_builder");
        let mut builder = PlayerBuilder::new();
        runlog::log_important("init: renderer backend");
        builder = builder.with_renderer(backend.clone());
        runlog::log_important("init: audio backend");
        builder = builder.with_audio(NullAudioBackend::new());
        #[cfg(feature = "navigator")]
        {
            runlog::log_important("init: navigator backend");
            builder = builder.with_navigator(backend.clone());
        }
        #[cfg(not(feature = "navigator"))]
        {
            runlog::log_important("init: navigator backend disabled");
        }
        #[cfg(feature = "storage")]
        {
            runlog::log_important("init: storage backend");
            builder = builder.with_storage(Box::new(backend.clone()));
        }
        #[cfg(not(feature = "storage"))]
        {
            runlog::log_important("init: storage backend disabled");
        }
        runlog::log_important("init: video backend");
        builder = builder.with_video(NullVideoBackend::new());
        runlog::log_important("init: log backend");
        builder = builder.with_log(backend.clone());
        runlog::log_important("init: ui backend");
        let player = builder.with_ui(backend.clone()).build();

        // Load SWF.
        match SwfMovie::from_data(&movie_bytes, root_file_url.clone(), None) {
            Ok(movie) => {
                backend.mark_movie_loaded(movie.version());
                runlog::log_important(&format!("Engine::new SwfMovie ok version={}", movie.version()));
                player.lock().unwrap().mutate_with_update_context(|uc| {
                    uc.set_root_movie(movie);
                });
                player.lock().unwrap().set_is_playing(true);
            }
            Err(e) => {
                backend.set_fatal_error(format!("Ruffle refused SWF: {e:?}"));
                runlog::warn_line(&format!("fatal: Ruffle refused SWF: {e:?}"));
                return Err(format!("Ruffle refused SWF: {e:?}"));
            }
        }

        Ok(Self {
            player,
            backend,
            renderer: Renderer::new(caches),
            scratch_packet: FramePacket::new(),
            root_path,
            root_file_url,
            frame_counter: 0,
            last_heartbeat_ms: 0,
            pending_snapshot: None,
        })
    }

    /// Tick Ruffle and render the latest submitted frame to the top framebuffer.
    ///
    /// This keeps the existing C-side loop unchanged: C calls `bridge_tick`, then swaps buffers.
    pub fn tick_and_render(&mut self) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        runlog::tick();
        if runlog::is_verbose() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if self.last_heartbeat_ms == 0 {
                self.last_heartbeat_ms = now;
            }
            if now.saturating_sub(self.last_heartbeat_ms) >= 30_000 {
                self.last_heartbeat_ms = now;
                runlog::log_line(&format!(
                    "tick heartbeat frames={} stage=submit_frame",
                    self.frame_counter
                ));
            }
        }
        // Poll any async-ish tasks queued by Ruffle backends.
        self.backend.poll_tasks();

        // 60 Hz fixed step for now.
        {
            let mut player = self.player.lock().unwrap();
            player.tick(1.0 / 60.0);
        }

        // Determine desired clear color.
        let clear = {
            let mut player = self.player.lock().unwrap();
            player.background_color().unwrap_or(Color { r: 0, g: 0, b: 0, a: 255 })
        };

        self.backend.begin_frame();

        if let Some(reason) = self.pending_snapshot.take() {
            let snap = format!("reason={}\n{}", reason, self.backend.status_snapshot_full());
            runlog::status_snapshot(&snap);
        }

        // Trigger Ruffle rendering; this will call our backend hooks.
        {
            runlog::stage("player.render", self.frame_counter);
            let mut player = self.player.lock().unwrap();
            player.render();
        }

        // Pull latest frame without allocating.
        self.backend.pull_latest_frame_into(&mut self.scratch_packet, clear);

        // Loading indicator until we see actual draw commands.
        if !self.backend.has_seen_real_draw() {
            self.scratch_packet.cmds.push(RenderCmd::DebugLoadingIndicator);
        }

        #[cfg(debug_assertions)]
        if self.backend.debug_affine_overlay_enabled() {
            let translate = Matrix2D { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 20.0, ty: 20.0 };
            let scale = Matrix2D { a: 1.4, b: 0.0, c: 0.0, d: 0.8, tx: 120.0, ty: 20.0 };
            let rotate = Matrix2D { a: 0.8660254, b: 0.5, c: -0.5, d: 0.8660254, tx: 230.0, ty: 40.0 };
            let shear = Matrix2D { a: 1.0, b: 0.25, c: 0.35, d: 1.0, tx: 60.0, ty: 130.0 };
            self.scratch_packet.cmds.push(RenderCmd::DebugAffineRect { transform: translate, r: 220, g: 80, b: 80 });
            self.scratch_packet.cmds.push(RenderCmd::DebugAffineRect { transform: scale, r: 80, g: 200, b: 80 });
            self.scratch_packet.cmds.push(RenderCmd::DebugAffineRect { transform: rotate, r: 80, g: 120, b: 220 });
            self.scratch_packet.cmds.push(RenderCmd::DebugAffineRect { transform: shear, r: 220, g: 200, b: 80 });
        }

        runlog::stage("renderer.render", self.frame_counter);
        self.renderer.render(&self.scratch_packet);
        runlog::stage("present", self.frame_counter);
    }

    /// Append a short status snapshot to the SD run bundle.
    pub fn request_status_snapshot(&mut self, reason: &str) {
        if self.pending_snapshot.is_none() {
            self.pending_snapshot = Some(reason.to_string());
        }
    }

    /// Graceful shutdown hook (flush run bundle files).
    pub fn shutdown(&mut self) {
        runlog::log_line("Engine shutdown");
        runlog::shutdown();
    }

    pub fn request_command_dump(&mut self) {
        self.backend.request_command_dump();
    }

    pub fn toggle_wireframe_once(&mut self) {
        self.backend.toggle_wireframe_once();
    }

    pub fn set_wireframe_hold(&mut self, enabled: bool) {
        self.backend.set_wireframe_hold(enabled);
    }

    pub fn toggle_debug_affine_overlay(&mut self) -> bool {
        self.backend.toggle_debug_affine_overlay()
    }

    pub fn is_ready(&self) -> bool {
        self.backend.is_ready()
    }

    pub fn status_text(&self) -> String {
        // Keep it short: it will be printed every frame on the bottom screen.
        self.backend.status_text_short()
    }
}

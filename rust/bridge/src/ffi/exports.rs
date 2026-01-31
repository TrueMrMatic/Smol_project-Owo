use core::ffi::c_char;
use std::sync::{Mutex, OnceLock};

use crate::engine::Engine;
use crate::ffi::types::{cstr_to_string, write_c_string};
use crate::runlog;

#[no_mangle]
pub extern "C" fn bridge_runlog_drain(out: *mut c_char, out_len: u32) -> u32 {
    if out.is_null() || out_len == 0 { return 0; }
    // Safety: caller provides valid buffer.
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, out_len as usize) };
    let n = runlog::drain_console(buf);
    // Ensure NUL termination if room
    if n < buf.len() {
        buf[n] = 0;
    } else if !buf.is_empty() {
        buf[buf.len()-1] = 0;
    }
    n as u32
}


/// Opaque handle passed to C.
///
/// Design rule: C must treat this as an opaque pointer.
pub struct BridgeContext {
    engine: Engine,
}

static LAST_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn set_last_error(msg: String) {
    let lock = LAST_ERROR.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        *guard = Some(msg);
    }
}

fn take_last_error() -> Option<String> {
    let lock = LAST_ERROR.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        guard.take()
    } else {
        None
    }
}

fn normalize_sd_path(mut p: String) -> String {
    if let Some(rest) = p.strip_prefix("file:///") {
        p = rest.to_string();
    } else if let Some(rest) = p.strip_prefix("file://") {
        p = rest.to_string();
    }

    if !p.starts_with("sdmc:/") && !p.starts_with("romfs:/") {
        p = format!("sdmc:/{}", p.trim_start_matches('/'));
    }
    p
}

#[no_mangle]
pub extern "C" fn bridge_player_create_with_url(url: *const c_char) -> *mut BridgeContext {
    crate::util::logging::init_logger();

    let root = cstr_to_string(url).unwrap_or_else(|| "sdmc:/3ds/".to_string());
    let root = if root.trim().is_empty() {
        "sdmc:/3ds/".to_string()
    } else {
        root
    };

    let root_path = normalize_sd_path(root);

    match Engine::new(&root_path, 400, 240) {
        Ok(engine) => Box::into_raw(Box::new(BridgeContext { engine })),
        Err(err) => {
            set_last_error(err);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn bridge_engine_create(swf_path: *const c_char, screen_w: i32, screen_h: i32) -> *mut BridgeContext {
    crate::util::logging::init_logger();

    let root = cstr_to_string(swf_path).unwrap_or_else(|| "sdmc:/3ds/".to_string());
    let root = if root.trim().is_empty() {
        "sdmc:/3ds/".to_string()
    } else {
        root
    };

    let root_path = normalize_sd_path(root);
    let width = screen_w.max(1) as u32;
    let height = screen_h.max(1) as u32;

    match Engine::new(&root_path, width, height) {
        Ok(engine) => Box::into_raw(Box::new(BridgeContext { engine })),
        Err(err) => {
            set_last_error(err);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn bridge_engine_last_error(out: *mut c_char, out_len: u32) -> u32 {
    if out.is_null() || out_len == 0 {
        return 0;
    }
    let msg = take_last_error().unwrap_or_else(|| "Unknown error".to_string());
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, out_len as usize) };
    let mut written = 0usize;
    let bytes = msg.as_bytes();
    let copy_len = bytes.len().min(buf.len().saturating_sub(1));
    if copy_len > 0 {
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        written = copy_len;
    }
    buf[written] = 0;
    written as u32
}

#[no_mangle]
pub extern "C" fn bridge_player_destroy(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctxm = unsafe { &mut *ctx }; 
    ctxm.engine.shutdown();
    unsafe {
        drop(Box::from_raw(ctx));
    }
}

#[no_mangle]
pub extern "C" fn bridge_engine_destroy(ctx: *mut BridgeContext) {
    bridge_player_destroy(ctx);
}

#[no_mangle]
pub extern "C" fn bridge_tick(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.tick_and_render(16);
}

#[no_mangle]
pub extern "C" fn bridge_engine_tick(ctx: *mut BridgeContext, dt_ms: u32) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.tick_and_render(dt_ms);
}

#[no_mangle]
pub extern "C" fn bridge_engine_mouse_move(ctx: *mut BridgeContext, x: i32, y: i32) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.mouse_move(x, y);
}

#[no_mangle]
pub extern "C" fn bridge_engine_mouse_button(ctx: *mut BridgeContext, button: i32, down: bool) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.mouse_button(button, down);
}

#[no_mangle]
pub extern "C" fn bridge_engine_key(ctx: *mut BridgeContext, keycode: i32, down: bool) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.key_event(keycode, down);
}

#[no_mangle]
pub extern "C" fn bridge_print_status(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    println!("{}", ctx.engine.status_text());
}

/// Append a short status snapshot to the SD run bundle.
#[no_mangle]
pub extern "C" fn bridge_write_status_snapshot_ctx(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.request_status_snapshot("user");
}


/// Request one-time command dump on the next `submit_frame`.
#[no_mangle]
pub extern "C" fn bridge_request_command_dump_ctx(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.request_command_dump();
}

#[no_mangle]
pub extern "C" fn bridge_renderer_ready_ctx(ctx: *mut BridgeContext) -> u32 {
    if ctx.is_null() {
        return 0;
    }
    let ctx = unsafe { &mut *ctx };
    if ctx.engine.is_ready() { 1 } else { 0 }
}

/// Returns the number of bytes written (excluding the NUL terminator).
#[no_mangle]
pub extern "C" fn bridge_get_status_text(ctx: *mut BridgeContext, out: *mut c_char, cap: usize) -> usize {
    if ctx.is_null() {
        return 0;
    }
    let ctx = unsafe { &mut *ctx };
    let s = ctx.engine.status_text();
    write_c_string(out, cap, &s)
}


#[no_mangle]
pub extern "C" fn bridge_toggle_wireframe_once_ctx(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.toggle_wireframe_once();
}

#[no_mangle]
pub extern "C" fn bridge_set_wireframe_hold_ctx(ctx: *mut BridgeContext, enabled: i32) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.set_wireframe_hold(enabled != 0);
}

#[no_mangle]
pub extern "C" fn bridge_toggle_affine_debug_overlay_ctx(ctx: *mut BridgeContext) -> u32 {
    if ctx.is_null() {
        return 0;
    }
    let ctx = unsafe { &mut *ctx };
    if ctx.engine.toggle_debug_affine_overlay() { 1 } else { 0 }
}

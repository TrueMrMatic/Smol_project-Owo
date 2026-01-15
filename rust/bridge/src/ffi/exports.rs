use core::ffi::c_char;

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

    match Engine::new(&root_path) {
        Ok(engine) => Box::into_raw(Box::new(BridgeContext { engine })),
        Err(_) => core::ptr::null_mut(),
    }
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
pub extern "C" fn bridge_tick(ctx: *mut BridgeContext) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.engine.tick_and_render();
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

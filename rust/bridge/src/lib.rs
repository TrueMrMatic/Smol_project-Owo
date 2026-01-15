//! bridge (staticlib)
//!
//! This crate exposes a small C ABI for the 3DS homebrew app.
//!
//! Design rule: keep this file thin.

mod ffi;
mod engine;
mod ruffle_adapter;
mod render;
mod util;
mod runlog;

// Export C ABI symbols.
pub use ffi::exports::*;

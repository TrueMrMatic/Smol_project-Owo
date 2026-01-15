use core::ffi::c_char;

/// Copy a C string into a Rust `String`.
pub fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // Safety: caller promises `ptr` is a valid NUL-terminated string.
    let s = unsafe { std::ffi::CStr::from_ptr(ptr) };
    Some(s.to_string_lossy().into_owned())
}

/// Write a Rust string into a C buffer (NUL-terminated).
/// Returns the number of bytes written (excluding the final NUL).
pub fn write_c_string(out: *mut c_char, cap: usize, s: &str) -> usize {
    if out.is_null() || cap == 0 {
        return 0;
    }

    let bytes = s.as_bytes();
    let n = bytes.len().min(cap.saturating_sub(1));

    // Safety: caller provided writable memory for `cap` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, n);
        *out.add(n) = 0;
    }

    n
}

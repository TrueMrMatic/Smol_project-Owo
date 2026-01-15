use core::ffi::c_char;
use std::ffi::CString;

extern "C" {
    fn bridge_read_file(path: *const c_char, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32;
    fn bridge_free_file(ptr: *mut u8, len: usize);
}

pub fn read_file_bytes(path: &str) -> Option<Vec<u8>> {
    let c_path = CString::new(path).ok()?;
    let mut out_ptr: *mut u8 = core::ptr::null_mut();
    let mut out_len: usize = 0;

    let rc = unsafe { bridge_read_file(c_path.as_ptr(), &mut out_ptr, &mut out_len) };
    if rc != 0 || out_ptr.is_null() || out_len == 0 {
        return None;
    }

    let bytes = unsafe { core::slice::from_raw_parts(out_ptr as *const u8, out_len) }.to_vec();
    unsafe { bridge_free_file(out_ptr, out_len) };
    Some(bytes)
}

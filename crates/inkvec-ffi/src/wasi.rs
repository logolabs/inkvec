//! Buffers for a WebAssembly host, compiled only for WASI (`wasm32-wasip1`).
//!
//! There the library is a WebAssembly module rather than a shared library, and the host --
//! the Go package through wazero -- cannot hand the module a pointer into its own memory. It
//! asks for buffers in the module's memory instead, copies the image, the options (a
//! NUL-terminated string) and an `InkvecResult` into them, calls `inkvec_trace` exactly as a
//! C caller would, reads the result, calls `inkvec_result_free`, and gives the buffers back.
//! Everything else the host calls is the C ABI above, unchanged.
//!
//! Not in `include/inkvec.h`: a native caller allocates its own memory.

use std::alloc::{alloc, dealloc, Layout};

/// Alignment of every buffer: enough for `InkvecResult` and any scalar.
const ALIGN: usize = 16;

/// A buffer of `len` bytes in this module's memory, uninitialised; NULL when `len` is 0 or
/// the memory cannot grow. Release it with `inkvec_dealloc(ptr, len)`.
#[no_mangle]
extern "C" fn inkvec_alloc(len: usize) -> *mut u8 {
    match Layout::from_size_align(len, ALIGN) {
        // SAFETY: the layout has a nonzero size.
        Ok(layout) if len > 0 => unsafe { alloc(layout) },
        _ => std::ptr::null_mut(),
    }
}

/// Release a buffer from `inkvec_alloc`. `len` must be the length it was allocated with.
/// NULL, or a `len` of 0, does nothing.
///
/// # Safety
///
/// `ptr` is NULL or came from `inkvec_alloc(len)` and has not been released since.
#[no_mangle]
unsafe extern "C" fn inkvec_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    if let Ok(layout) = Layout::from_size_align(len, ALIGN) {
        // SAFETY: allocated by `inkvec_alloc` with this same layout, per the caller.
        unsafe { dealloc(ptr, layout) }
    }
}

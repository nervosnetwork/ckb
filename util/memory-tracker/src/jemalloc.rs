use ckb_logger::info;
use std::{ffi, mem, ptr};

/// Dumps the heap through Jemalloc's API.
///
/// This funcions works when the following conditions are satisfied:
/// - the global allocator is [Jemallocator].
/// - the profiling is enabled.
///
/// [Jemallocator]: https://docs.rs/jemallocator/*/jemallocator/index.html
pub fn jemalloc_profiling_dump(filename: &str) -> Result<(), String> {
    let mut filename0 = format!("{}\0", filename);
    let opt_name = "prof.dump";
    let opt_c_name = ffi::CString::new(opt_name).unwrap();
    info!("jemalloc profiling dump: {}", filename);
    unsafe {
        jemalloc_sys::mallctl(
            opt_c_name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut filename0 as *mut _ as *mut _,
            mem::size_of::<*mut ffi::c_void>(),
        );
    }

    Ok(())
}

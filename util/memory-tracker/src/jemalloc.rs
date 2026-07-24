use ckb_logger::info;
use std::{ffi, io, mem, ptr};

/// Dumps the heap through Jemalloc's API.
///
/// This functions works when the following conditions are satisfied:
/// - the global allocator is [Jemallocator].
/// - the profiling is enabled.
///
/// [Jemallocator]: https://docs.rs/jemallocator/*/jemallocator/index.html
pub fn jemalloc_profiling_dump(filename: &str) -> Result<(), String> {
    let filename_c = ffi::CString::new(filename)
        .map_err(|err| format!("invalid jemalloc profiling dump filename: {err}"))?;
    let mut filename_ptr = filename_c.as_ptr();
    let opt_name = "prof.dump";
    let opt_c_name = ffi::CString::new(opt_name).unwrap();
    info!("jemalloc profiling dump: {filename}");
    let result = unsafe {
        jemalloc_sys::mallctl(
            opt_c_name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::from_mut(&mut filename_ptr).cast(),
            mem::size_of_val(&filename_ptr),
        )
    };

    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "jemalloc profiling dump failed: {}",
            io::Error::from_raw_os_error(result)
        ))
    }
}

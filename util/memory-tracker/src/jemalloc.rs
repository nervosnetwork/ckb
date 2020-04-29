use ckb_logger::info;
use std::{ffi, mem, ptr};

pub fn jemalloc_profiling_dump(mut filename: String) {
    let opt_name = "prof.dump";
    let opt_c_name = ffi::CString::new(opt_name).unwrap();
    info!("jemalloc profiling dump: {}", filename);
    unsafe {
        jemalloc_sys::mallctl(
            opt_c_name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut filename as *mut _ as *mut _,
            mem::size_of::<*mut ffi::c_void>(),
        );
    }
}

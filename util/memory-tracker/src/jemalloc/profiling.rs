#[cfg(all(
    not(target_env = "msvc"),
    not(target_os = "macos"),
    not(feature = "disable-jemalloc"),
    feature = "profiling"
))]
pub fn jemalloc_profiling_dump(filename: &str) -> Result<(), String> {
    use ckb_logger::info;
    use std::{ffi, mem, ptr};
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

#[cfg(any(
    target_env = "msvc",
    target_os = "macos",
    feature = "disable-jemalloc",
    not(feature = "profiling")
))]
pub fn jemalloc_profiling_dump(_: &str) -> Result<(), String> {
    Err("jemalloc profiling dump: unsupported".to_string())
}

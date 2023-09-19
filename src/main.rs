//! CKB executable main entry.
use ckb_bin::run_app;
use ckb_build_info::Version;

#[cfg(all(not(target_env = "msvc"), not(target_os = "macos")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
use ckb_async_runtime::new_global_runtime;

fn main() {
    let version = get_version();
    let (handle, _handle_stop_rx, runtime) = new_global_runtime();
    if let Some(exit_code) = runtime.block_on(run_app(version, handle)).err() {
        ::std::process::exit(exit_code.into());
    }
}

fn get_version() -> Version {
    let major = env!("CARGO_PKG_VERSION_MAJOR")
        .parse::<u8>()
        .expect("CARGO_PKG_VERSION_MAJOR parse success");
    let minor = env!("CARGO_PKG_VERSION_MINOR")
        .parse::<u8>()
        .expect("CARGO_PKG_VERSION_MINOR parse success");
    let patch = env!("CARGO_PKG_VERSION_PATCH")
        .parse::<u16>()
        .expect("CARGO_PKG_VERSION_PATCH parse success");
    let dash_pre = {
        let pre = env!("CARGO_PKG_VERSION_PRE");
        if pre.is_empty() {
            pre.to_string()
        } else {
            "-".to_string() + pre
        }
    };

    let commit_describe = option_env!("COMMIT_DESCRIBE").map(ToString::to_string);
    #[cfg(docker)]
    let commit_describe = commit_describe.map(|s| s.replace("-dirty", ""));
    let commit_date = option_env!("COMMIT_DATE").map(ToString::to_string);
    let code_name = None;
    Version {
        major,
        minor,
        patch,
        dash_pre,
        code_name,
        commit_describe,
        commit_date,
    }
}

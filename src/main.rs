//! CKB executable main entry.
use ckb_bin::run_app;
use ckb_build_info::Version;

#[cfg(all(not(target_env = "msvc"), not(target_os = "macos")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    #[cfg(feature = "tokio-trace")]
    console_subscriber::init();

    #[cfg(target_os = "windows")]
    check_msvc_version();

    let version = get_version();
    if let Some(exit_code) = run_app(version).err() {
        ::std::process::exit(exit_code.into());
    }
}

#[cfg(target_os = "windows")]
fn check_msvc_version() {
    use winreg::RegKey;
    use winreg::enums::*;
    // if users msvc version less than 14.44, print a warning

    fn get_vc_redist_version(arch: &str) -> std::io::Result<Option<String>> {
        // arch: "x64" or "x86"
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let key_path = format!(
            r"SOFTWARE\Wow6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\{}",
            arch
        );
        match hklm.open_subkey(&key_path) {
            Ok(key) => {
                let version: String = key.get_value("Version")?;
                Ok(Some(version))
            }
            Err(_) => Ok(None),
        }
    }

    fn version_string_to_tuple(ver: &str) -> Option<(u32, u32, u32, u32)> {
        let parts: Vec<&str> = ver.split('.').collect();
        if parts.len() >= 4 {
            let major = parts[0].parse().ok()?;
            let minor = parts[1].parse().ok()?;
            let bld = parts[2].parse().ok()?;
            let rbld = parts[3].parse().ok()?;
            Some((major, minor, bld, rbld))
        } else {
            None
        }
    }
    fn is_version_at_least(current: &str, threshold: &str) -> bool {
        if let (Some(cur), Some(thr)) = (
            version_string_to_tuple(current),
            version_string_to_tuple(threshold),
        ) {
            cur >= thr
        } else {
            false
        }
    }

    if let Some(version) = get_vc_redist_version("x64").unwrap_or_default() {
        eprintln!("Detected VC++ Redistributable version (x64): {}", version);
        let threshold = "14.44.0.0";
        if !is_version_at_least(&version, threshold) {
            eprintln!(
                "Version is below {}. Please download/upgrade the Visual C++ Redistributable. Help: https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170#latest-supported-redistributable-version ",
                threshold
            );
        } else {
            eprintln!("MSVC Version: {} meets the requirement.", version);
        }
    } else {
        eprintln!(
            "Visual C++ Redistributable version not found. Please install it. Help: https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170#latest-supported-redistributable-version"
        );
    }
}

#[allow(unexpected_cfgs)]
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

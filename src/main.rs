//! CKB executable main entry.
use ckb_bin::run_app;
use ckb_build_info::Version;

#[cfg(all(feature = "tokio-trace", not(tokio_unstable)))]
compile_error!(
    "the `tokio-trace` feature requires `RUSTFLAGS=\"--cfg tokio_unstable\"` at compile time"
);

#[cfg(all(not(target_env = "msvc"), not(target_os = "macos")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    #[cfg(any(feature = "profiling", feature = "tokio-trace"))]
    if let Err(error) = init_optional_observability() {
        eprintln!("optional profiling initialization failed: {error}");
    }

    #[cfg(all(target_os = "windows", not(target_feature = "crt-static")))]
    check_msvc_version();

    let version = get_version();
    if let Some(exit_code) = run_app(version).err() {
        ::std::process::exit(exit_code.into());
    }
}

#[cfg(any(feature = "profiling", feature = "tokio-trace"))]
fn init_optional_observability() -> Result<(), Box<dyn std::error::Error>> {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::FilterFn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    fn optional_env(name: &str) -> Result<Option<String>, std::env::VarError> {
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[cfg(feature = "profiling")]
    let tx_pool_layer = if let Some(path) = optional_env("TX_POOL_PROFILE_TRACE_PATH")? {
        let output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let filter = FilterFn::new(|metadata| metadata.target() == "ckb_tx_pool_profile");
        Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
                .with_writer(std::sync::Arc::new(output))
                .with_filter(filter),
        )
    } else {
        None
    };

    #[cfg(all(feature = "profiling", not(feature = "tokio-trace")))]
    if tx_pool_layer.is_none() {
        return Ok(());
    }

    #[cfg(feature = "tokio-trace")]
    let (console_layer, start_tx) = {
        use std::net::ToSocketAddrs;

        fn positive_usize(name: &str, value: &str) -> Result<usize, std::io::Error> {
            let parsed = value.parse::<usize>().map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{name} must be a positive integer: {error}"),
                )
            })?;
            if parsed == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{name} must be greater than zero"),
                ));
            }
            Ok(parsed)
        }

        fn positive_duration(
            name: &str,
            value: &str,
        ) -> Result<std::time::Duration, std::io::Error> {
            let parsed = humantime::parse_duration(value).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{name} must be a duration: {error}"),
                )
            })?;
            if parsed.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("{name} must be greater than zero"),
                ));
            }
            Ok(parsed)
        }

        let mut builder = console_subscriber::ConsoleLayer::builder();
        if let Some(value) = optional_env("TOKIO_CONSOLE_RETENTION")? {
            builder = builder.retention(humantime::parse_duration(&value)?);
        }
        if let Some(value) = optional_env("TOKIO_CONSOLE_PUBLISH_INTERVAL")? {
            builder = builder
                .publish_interval(positive_duration("TOKIO_CONSOLE_PUBLISH_INTERVAL", &value)?);
        }
        if let Some(value) = optional_env("TOKIO_CONSOLE_BIND")? {
            let address = value.to_socket_addrs()?.next().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("TOKIO_CONSOLE_BIND resolved to no address: {value}"),
                )
            })?;
            builder = builder.server_addr(address);
        }
        if optional_env("TOKIO_CONSOLE_RECORD_PATH")?.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "TOKIO_CONSOLE_RECORD_PATH is disabled because console-subscriber opens it with a panic-based API",
            )
            .into());
        }
        if let Some(value) = optional_env("TOKIO_CONSOLE_BUFFER_CAPACITY")? {
            builder = builder
                .event_buffer_capacity(positive_usize("TOKIO_CONSOLE_BUFFER_CAPACITY", &value)?);
        }

        let (layer, server) = builder.build();
        let filter = FilterFn::new(|metadata| {
            if metadata.is_event() {
                metadata.target().starts_with("runtime") || metadata.target().starts_with("tokio")
            } else {
                metadata.name().starts_with("runtime.") || metadata.target().starts_with("tokio")
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let (start_tx, start_rx) = std::sync::mpsc::sync_channel(0);
        std::thread::Builder::new()
            .name("console_subscriber".into())
            .spawn(move || {
                if start_rx.recv().is_err() {
                    return;
                }
                if let Err(error) = runtime.block_on(server.serve()) {
                    eprintln!("tokio-console server stopped: {error}");
                }
            })?;
        (layer.with_filter(filter), start_tx)
    };

    let subscriber = tracing_subscriber::registry();
    #[cfg(feature = "profiling")]
    let subscriber = subscriber.with(tx_pool_layer);
    #[cfg(feature = "tokio-trace")]
    let subscriber = subscriber.with(console_layer);
    subscriber.try_init()?;

    #[cfg(feature = "tokio-trace")]
    {
        // The server thread remains gated until the global subscriber has
        // been installed successfully.
        start_tx.send(()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "tokio-console server thread exited before startup",
            )
        })?;
    }
    Ok(())
}

#[cfg(all(target_os = "windows", not(target_feature = "crt-static")))]
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

    fn is_version_at_least(current: &str, threshold: &str) -> bool {
        use version_compare::{Cmp, Version};

        // Strip leading 'v' or 'V' if present
        let current = current.trim_start_matches(|c| c == 'v' || c == 'V');
        let threshold = threshold.trim_start_matches(|c| c == 'v' || c == 'V');

        if let (Some(cur), Some(thr)) = (Version::from(current), Version::from(threshold)) {
            cur.compare(&thr) != Cmp::Lt
        } else {
            false
        }
    }

    if let Some(version) = get_vc_redist_version("x64").unwrap_or_default() {
        let threshold = "14.44.0.0";
        if !is_version_at_least(&version, threshold) {
            eprintln!("Detected VC++ Redistributable version (x64): {}", version);
            eprintln!(
                "Version is below {}. Please download/upgrade the Visual C++ Redistributable. Help: https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170#latest-supported-redistributable-version ",
                threshold
            );
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

//! Optional process memory snapshots at workload boundaries, outside timing.
//! High-water marks retain earlier peaks; differences do not measure phase cost.

#[derive(Default)]
pub(crate) struct ResourcePhases {
    enabled: bool,
    observations: Vec<serde_json::Value>,
}

impl ResourcePhases {
    pub(crate) fn from_environment() -> Result<Self, String> {
        let enabled = match std::env::var("TX_POOL_BENCH_RESOURCE_PHASES") {
            Err(std::env::VarError::NotPresent) => false,
            Ok(value) if value == "0" => false,
            Ok(value) if value == "1" => true,
            _ => return Err("TX_POOL_BENCH_RESOURCE_PHASES must be 0 or 1".into()),
        };
        if enabled {
            println!("BENCH_DIAGNOSTICS resource_phases=true");
        }
        Ok(Self {
            enabled,
            ..Self::default()
        })
    }

    pub(crate) fn capture(&mut self, phase: &str) -> Result<(), String> {
        if self.enabled {
            let (resident, high_water) = memory_bytes()?;
            self.observations.push(serde_json::json!({
                "phase": phase,
                "resident_bytes": resident,
                "lifetime_peak_rss_bytes": high_water,
            }));
        }
        Ok(())
    }

    pub(crate) fn finish(&self) {
        if self.enabled {
            println!(
                "BENCH_RESOURCE_PHASES {}",
                serde_json::json!({
                    "schema_version": 1,
                    "scope": "process snapshots; cumulative peaks cannot be subtracted into phase costs",
                    "observations": self.observations,
                })
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(
    deprecated,
    reason = "libc deprecates its Mach bindings in favor of another crate; this diagnostic uses the existing binding to the supported OS API."
)]
fn memory_bytes() -> Result<(u64, u64), String> {
    let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::uninit();
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: the flavor and count describe this exact output structure;
    // task_info initializes it on success. The port belongs to this process.
    let result = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS || count != libc::MACH_TASK_BASIC_INFO_COUNT {
        return Err(format!(
            "process task_info failed: result={result} count={count}"
        ));
    }
    // SAFETY: successful task_info and the complete returned count were checked.
    let info = unsafe { info.assume_init() };
    Ok((info.resident_size, info.resident_size_max))
}

#[cfg(target_os = "linux")]
fn memory_bytes() -> Result<(u64, u64), String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|error| error.to_string())?;
    let field = |name| -> Result<u64, String> {
        let line = status
            .lines()
            .find(|line| line.starts_with(name))
            .ok_or(name)?;
        let mut fields = line.split_whitespace().skip(1);
        let kib = fields
            .next()
            .ok_or(name)?
            .parse::<u64>()
            .map_err(|error| error.to_string())?;
        if fields.next() != Some("kB") || fields.next().is_some() {
            return Err(format!("unsupported {name} memory unit"));
        }
        kib.checked_mul(1_024)
            .ok_or_else(|| "process RSS overflow".into())
    };
    Ok((field("VmRSS:")?, field("VmHWM:")?))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn memory_bytes() -> Result<(u64, u64), String> {
    Err("resource phase observation requires macOS or Linux".into())
}

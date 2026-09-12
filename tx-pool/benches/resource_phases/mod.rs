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

mod memory;
use memory::memory_bytes;

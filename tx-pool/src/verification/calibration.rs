//! Machine calibration for network admission; never a consensus limit.
//!
//! Measures two fixed, reviewable synthetic kernels (ALU/stack and conditional
//! branch with 256 KiB page-strided memory traffic) across all supported VM
//! versions using only the thread's own CPU clock. Never learns from peer
//! transactions; periodic and pressure-triggered refreshes re-run only the
//! trusted synthetic kernels under a single-flight rate limit.

use super::cpu_clock::CpuTime;
use ckb_script::{ScriptVersion, types::Machine};
use ckb_types::bytes::Bytes;
use ckb_util::parking_lot::RwLock;
use ckb_vm::{
    DefaultMachineBuilder, DefaultMachineRunner, Error, SupportMachine,
    cost_model::estimate_cycles,
    elf::{LoadingAction, ProgramMetadata},
    memory::{FLAG_EXECUTABLE, FLAG_FREEZED},
};
use std::{
    num::NonZeroU128,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const SAMPLE_CYCLES: u64 = 5_000_000;
/// Periodic trusted recalibration cadence so nodes adapt if host CPU frequency
/// shifts (e.g. cloud burst credits or heterogeneous P/E-core migration).
pub(crate) const PROACTIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(600);
/// Rate-limited trusted recalibration cooldown after an actual network
/// `ExcessiveVerifyTime` refusal so throttling heals promptly without DoS
/// amplification (<0.05% CPU ceiling).
pub(crate) const PRESSURE_REFRESH_COOLDOWN: Duration = Duration::from_secs(60);

/// Machine-local measurements, without transaction-pool policy or peer input.
#[derive(Clone, Copy, Debug)]
struct Measurement {
    /// Executed consensus cycles in the fixed workload.
    cycles: u64,
    /// CPU time spent creating the machine and loading its program and stack.
    loading_time: Duration,
    /// CPU time spent running the loaded workload to its cycle limit.
    execution_time: Duration,
}

impl Measurement {
    /// Measure all supported script versions across two synthetic kernels using
    /// the node's actual VM backend.
    ///
    /// Three CPU-time samples per (kernel, version) suppress isolated scheduling
    /// disturbances; retain the slowest per-cycle execution rate and slowest
    /// loading time across both kernels and all supported versions. Neither
    /// kernel invokes syscalls or parses ELF headers, so callers keep a
    /// conservative safety margin and scale startup allowance by script groups.
    fn measure() -> Result<Self, Box<dyn std::error::Error>> {
        let kernels = [Self::alu_stack_kernel(), Self::branch_stride_kernel()];
        let mut slowest: Option<Self> = None;
        let mut max_loading = Duration::ZERO;
        for (program, metadata) in kernels {
            for version in [ScriptVersion::V0, ScriptVersion::V1, ScriptVersion::V2] {
                let sample = || Self::sample(version, &program, &metadata, SAMPLE_CYCLES);
                let mut samples = [sample()?, sample()?, sample()?];
                samples.sort_unstable_by_key(|sample| sample.execution_time);
                let median = samples[1];
                if slowest.is_none_or(|current| is_slower_rate(median, current)) {
                    slowest = Some(median);
                }
                samples.sort_unstable_by_key(|sample| sample.loading_time);
                max_loading = max_loading.max(samples[1].loading_time);
            }
        }
        let mut result = slowest.unwrap_or(Self {
            cycles: SAMPLE_CYCLES,
            loading_time: Duration::ZERO,
            execution_time: Duration::ZERO,
        });
        result.loading_time = max_loading;
        Ok(result)
    }

    fn alu_stack_kernel() -> (Bytes, ProgramMetadata) {
        // Keep the instructions reviewable without a compiled benchmark asset.
        // The arithmetic/load/store loop stops at the VM's cycle limit.
        let instructions: [u32; 6] = [
            0x0012_8293, // addi t0, t0, 1
            0xfe51_3c23, // sd   t0, -8(sp)
            0xff81_3303, // ld   t1, -8(sp)
            0x0262_83b3, // mul  t2, t0, t1
            0x0063_c2b3, // xor  t0, t2, t1
            0xfedf_f06f, // j    -20
        ];
        let mut program = vec![0; 64 * 1024];
        for (bytes, instruction) in program.chunks_exact_mut(4).zip(instructions) {
            bytes.copy_from_slice(&instruction.to_le_bytes());
        }
        let program = Bytes::from(program);
        let metadata = ProgramMetadata {
            actions: vec![LoadingAction {
                addr: 0,
                size: program.len() as u64,
                flags: FLAG_EXECUTABLE | FLAG_FREEZED,
                source: 0..program.len() as u64,
                offset_from_addr: 0,
            }],
            entry: 0,
        };
        (program, metadata)
    }

    fn branch_stride_kernel() -> (Bytes, ProgramMetadata) {
        // Base RV64I conditional-branch + multi-page strided memory loop
        // touching 64 * 4 KiB pages (256 KiB RW segment at 0x10_0000) so L1-hot
        // single-slot bias does not overstate VM throughput.
        let instructions: [u32; 10] = [
            0x0010_0e37, // lui  t3, 0x100     (base = 0x10_0000)
            0x0004_0eb7, // lui  t4, 0x40      (size = 0x04_0000 = 256 KiB)
            0xff8e_8e93, // addi t4, t4, -8    (8-byte aligned mask 0x03_fff8)
            0x005e_0f33, // add  t5, t3, t0    (loop: addr = base + offset)
            0x000f_3303, // ld   t1, 0(t5)
            0x0013_0313, // addi t1, t1, 1
            0x006f_3023, // sd   t1, 0(t5)
            0x4082_8293, // addi t0, t0, 1032  (crosses cache lines & 4 KiB pages)
            0x01d2_f2b3, // and  t0, t0, t4
            0xfe03_14e3, // bne  t1, zero, -24
        ];
        let mut program = vec![0; 64 * 1024];
        for (bytes, instruction) in program.chunks_exact_mut(4).zip(instructions) {
            bytes.copy_from_slice(&instruction.to_le_bytes());
        }
        let program = Bytes::from(program);
        let metadata = ProgramMetadata {
            actions: vec![
                LoadingAction {
                    addr: 0,
                    size: program.len() as u64,
                    flags: FLAG_EXECUTABLE | FLAG_FREEZED,
                    source: 0..program.len() as u64,
                    offset_from_addr: 0,
                },
                LoadingAction {
                    addr: 0x0010_0000,
                    size: 256 * 1024,
                    flags: 0,
                    source: 0..0,
                    offset_from_addr: 0,
                },
            ],
            entry: 0,
        };
        (program, metadata)
    }

    fn sample(
        version: ScriptVersion,
        program: &Bytes,
        metadata: &ProgramMetadata,
        cycles: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let started = CpuTime::now()?;
        let core = version.init_core_machine(cycles);
        let mut machine = Machine::new(
            DefaultMachineBuilder::new(core)
                .instruction_cycle_func(Box::new(estimate_cycles))
                .build(),
        );
        machine.load_program_with_metadata(program, metadata, std::iter::empty())?;
        let loading_time = started.elapsed()?;
        let started = CpuTime::now()?;
        match machine.run() {
            Err(Error::CyclesExceeded) => Ok(Self {
                cycles: machine.machine().cycles(),
                loading_time,
                execution_time: started.elapsed()?,
            }),
            Err(error) => Err(error.into()),
            Ok(_) => Err(Error::Unexpected("calibration loop exited".into()).into()),
        }
    }
}

/// True when `candidate` executes fewer cycles per nanosecond than `current`.
fn is_slower_rate(candidate: Measurement, current: Measurement) -> bool {
    let lhs = candidate
        .execution_time
        .as_nanos()
        .saturating_mul(u128::from(current.cycles));
    let rhs = current
        .execution_time
        .as_nanos()
        .saturating_mul(u128::from(candidate.cycles));
    lhs > rhs
}

#[derive(Clone, Copy)]
struct CalibrationEntry {
    timing: Option<VmTiming>,
    calibrated_at: Instant,
}

struct CalibrationStore {
    entry: RwLock<Option<CalibrationEntry>>,
    refreshing: AtomicBool,
}

impl CalibrationStore {
    const fn new() -> Self {
        Self {
            entry: RwLock::new(None),
            refreshing: AtomicBool::new(false),
        }
    }

    fn measure_and_build() -> Option<VmTiming> {
        let sample = match Measurement::measure() {
            Ok(sample) => sample,
            Err(error) => {
                ckb_logger::warn!("VM calibration failed; using the network time cap: {error}");
                return None;
            }
        };
        let Some(timing) = VmTiming::from_sample(sample) else {
            ckb_logger::warn!(
                "VM calibration clock resolution is insufficient; using the network time cap"
            );
            return None;
        };
        Some(timing)
    }

    fn ensure_initialized(&self) -> Option<VmTiming> {
        if let Some(entry) = *self.entry.read() {
            return entry.timing;
        }
        let timing = crate::util::block_offload(Self::measure_and_build);
        let mut guard = self.entry.write();
        if let Some(existing) = *guard {
            return existing.timing;
        }
        if let Some(timing) = timing {
            ckb_logger::info!(
                "Network VM CPU calibration: {} cycles/ms, per-group load {} us, minimum {} ms",
                timing.cycles_per_ms,
                timing.per_group_load.as_micros(),
                timing.minimum.as_millis(),
            );
            crate::metrics::record_verify_calibration_cycles_per_ms(timing.cycles_per_ms);
        }
        *guard = Some(CalibrationEntry {
            timing,
            calibrated_at: Instant::now(),
        });
        timing
    }

    fn try_begin_refresh(&self, min_age: Duration, now: Instant) -> Option<RefreshGuard<'_>> {
        let stale = self
            .entry
            .read()
            .is_some_and(|entry| now.saturating_duration_since(entry.calibrated_at) >= min_age);
        if !stale {
            return None;
        }
        if self
            .refreshing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }
        let still_stale = self
            .entry
            .read()
            .is_some_and(|entry| now.saturating_duration_since(entry.calibrated_at) >= min_age);
        if !still_stale {
            self.refreshing.store(false, Ordering::SeqCst);
            return None;
        }
        Some(RefreshGuard { store: self })
    }

    fn commit_refresh(&self, timing: Option<VmTiming>, now: Instant) {
        let previous = self.entry.read().and_then(|entry| entry.timing);
        if let Some(next) = timing {
            crate::metrics::record_verify_calibration_cycles_per_ms(next.cycles_per_ms);
            if let Some(prev) = previous {
                let delta = prev.cycles_per_ms.abs_diff(next.cycles_per_ms);
                if delta.saturating_mul(5) >= prev.cycles_per_ms {
                    ckb_logger::info!(
                        "Network VM CPU calibration refreshed: {} -> {} cycles/ms, minimum {} ms",
                        prev.cycles_per_ms,
                        next.cycles_per_ms,
                        next.minimum.as_millis(),
                    );
                }
            }
            *self.entry.write() = Some(CalibrationEntry {
                timing: Some(next),
                calibrated_at: now,
            });
        } else if let Some(entry) = self.entry.write().as_mut() {
            // Retain last valid calibration on a transient measurement failure,
            // advancing the timestamp so callers respect the refresh cooldown.
            entry.calibrated_at = now;
        }
    }
}

pub(crate) struct RefreshGuard<'a> {
    store: &'a CalibrationStore,
}

impl RefreshGuard<'_> {
    /// Execute a single trusted synthetic recalibration off the async runtime
    /// and publish the refreshed timing atomically.
    pub(crate) fn run(self) {
        let timing = CalibrationStore::measure_and_build();
        self.store.commit_refresh(timing, Instant::now());
    }
}

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.store.refreshing.store(false, Ordering::SeqCst);
    }
}

static VM_TIMING: CalibrationStore = CalibrationStore::new();

/// Calibrate before the pool accepts work; all generations share the result.
pub(crate) fn initialize() {
    let _ = VM_TIMING.ensure_initialized();
}

/// Claim a single-flight trusted recalibration slot if the current calibration
/// is at least `min_age` old. Returns `None` when fresh or already in flight.
pub(crate) fn try_begin_refresh(min_age: Duration) -> Option<RefreshGuard<'static>> {
    #[cfg(test)]
    if USE_NETWORK_CAP.get().is_some() {
        return None;
    }
    VM_TIMING.try_begin_refresh(min_age, Instant::now())
}

#[cfg(test)]
static USE_NETWORK_CAP: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Lifecycle tests use a fixed, bounded CPU budget instead of a hardware timing
/// assertion. Nextest isolates this choice from calibration and budget tests.
#[cfg(test)]
pub(crate) fn use_network_cap_for_test() {
    USE_NETWORK_CAP
        .set(())
        .expect("configure once before service startup");
}

#[cfg(test)]
pub(crate) fn for_cycles(cycles: u64, cap: Duration) -> Duration {
    for_workload(cycles, 1, cap)
}

pub(crate) fn for_workload(cycles: u64, script_groups: usize, cap: Duration) -> Duration {
    #[cfg(test)]
    if USE_NETWORK_CAP.get().is_some() {
        return cap;
    }
    VM_TIMING.ensure_initialized().map_or(cap, |timing| {
        timing.limit_for_groups(cycles, script_groups, cap)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VmTiming {
    cycles_per_ms: u64,
    per_group_load: Duration,
    minimum: Duration,
}

impl VmTiming {
    fn from_sample(sample: Measurement) -> Option<Self> {
        // Synthetic kernels omit syscalls, ELF parsing, and data-provider CPU,
        // and ASM backends can run tight loops substantially faster than real
        // scripts. Keep a conservative 16x slack across both measurements while
        // scaling per-group loading additively with script group count.
        const MARGIN: u32 = 16;
        let nanos = sample
            .execution_time
            .as_nanos()
            .saturating_mul(MARGIN.into());
        let nanos = NonZeroU128::new(nanos)?;
        let cycles_per_ms = u128::from(sample.cycles)
            .saturating_mul(1_000_000)
            .checked_div(nanos.get())?
            .clamp(1, u64::MAX.into()) as u64;
        let per_group_load = sample.loading_time.saturating_mul(MARGIN);
        let minimum = sample
            .loading_time
            .saturating_add(sample.execution_time)
            .saturating_mul(MARGIN);
        let millis = minimum
            .as_nanos()
            .div_ceil(1_000_000)
            .clamp(1, u64::MAX.into());
        Some(Self {
            cycles_per_ms,
            per_group_load,
            minimum: Duration::from_millis(millis as u64),
        })
    }

    fn limit_for_groups(&self, cycles: u64, script_groups: usize, cap: Duration) -> Duration {
        let groups = u32::try_from(script_groups.max(1)).unwrap_or(u32::MAX);
        let startup = self.per_group_load.saturating_mul(groups);
        let execution = Duration::from_millis(cycles.div_ceil(self.cycles_per_ms));
        startup.saturating_add(execution).max(self.minimum).min(cap)
    }

    #[cfg(test)]
    fn limit(&self, cycles: u64, cap: Duration) -> Duration {
        self.limit_for_groups(cycles, 1, cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_budget_stays_bounded_by_the_configured_cap() {
        use_network_cap_for_test();
        for cap in [
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_secs(8),
        ] {
            for cycles in [0, 1, SAMPLE_CYCLES, u64::MAX] {
                assert_eq!(for_cycles(cycles, cap), cap);
                assert_eq!(for_workload(cycles, 32, cap), cap);
            }
        }
    }

    /// Hardware observations run separately from concurrent unit tests. The
    /// holdouts exercise a short branch loop and a larger division/remainder
    /// program, at cycle counts below and above the calibration quantum.
    #[cfg(feature = "calibration-observation")]
    #[test]
    #[ignore = "hardware observation: run alone with the prod profile and --nocapture"]
    fn observe_calibration_against_holdout_workloads() {
        let calibration = Measurement::measure().unwrap();
        let timing = VmTiming::from_sample(calibration).unwrap();
        println!(
            "CALIBRATION_SAMPLE {}",
            serde_json::json!({
                "backend": std::any::type_name::<Machine>(),
                "cycles": calibration.cycles,
                "loading_ns": calibration.loading_time.as_nanos(),
                "execution_ns": calibration.execution_time.as_nanos(),
                "cycles_per_ms": timing.cycles_per_ms,
                "per_group_load_ns": timing.per_group_load.as_nanos(),
                "minimum_ms": timing.minimum.as_millis(),
            })
        );
        let workloads: [(&str, &[u32], usize, u64); 2] = [
            (
                "branch",
                &[0x0012_8293, 0xffdf_f06f], // addi t0,t0,1; j -4
                4 * 1024,
                1_000_000,
            ),
            (
                "division",
                &[
                    0x0012_8293, // addi t0,t0,1
                    0x0252_c333, // div t1,t0,t0
                    0x0253_63b3, // rem t2,t1,t0
                    0xff5f_f06f, // j -12
                ],
                1024 * 1024,
                50_000_000,
            ),
        ];
        for (name, instructions, size, cycles) in workloads {
            let mut program = vec![0; size];
            for (bytes, instruction) in program.chunks_exact_mut(4).zip(instructions) {
                bytes.copy_from_slice(&instruction.to_le_bytes());
            }
            let program = Bytes::from(program);
            let metadata = ProgramMetadata {
                actions: vec![LoadingAction {
                    addr: 0,
                    size: size as u64,
                    flags: FLAG_EXECUTABLE | FLAG_FREEZED,
                    source: 0..size as u64,
                    offset_from_addr: 0,
                }],
                entry: 0,
            };
            for version in [ScriptVersion::V0, ScriptVersion::V1, ScriptVersion::V2] {
                for repeat in 0..3 {
                    let sample = Measurement::sample(version, &program, &metadata, cycles)
                        .expect("the holdout must execute to its cycle limit");
                    let elapsed = sample.loading_time + sample.execution_time;
                    let budget = timing.limit(cycles, Duration::from_secs(8));
                    println!(
                        "CALIBRATION_HOLDOUT {}",
                        serde_json::json!({
                            "workload": name, "version": format!("{version:?}"),
                            "repeat": repeat, "program_bytes": size,
                            "declared_cycles": cycles, "executed_cycles": sample.cycles,
                            "loading_ns": sample.loading_time.as_nanos(),
                            "execution_ns": sample.execution_time.as_nanos(),
                            "budget_ns": budget.as_nanos(),
                            "within_budget": elapsed <= budget,
                        })
                    );
                    assert!(sample.cycles > cycles - 32 && sample.cycles <= cycles);
                    assert!(
                        elapsed <= budget,
                        "{name} {version:?}: {elapsed:?} > {budget:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn fixed_workloads_run_to_their_cycle_limit_on_every_script_version() {
        let sample = Measurement::measure().unwrap();
        assert!(sample.cycles > SAMPLE_CYCLES - 32);
        assert!(sample.cycles <= SAMPLE_CYCLES);
        for (program, metadata) in [
            Measurement::alu_stack_kernel(),
            Measurement::branch_stride_kernel(),
        ] {
            for version in [ScriptVersion::V0, ScriptVersion::V1, ScriptVersion::V2] {
                let s = Measurement::sample(version, &program, &metadata, SAMPLE_CYCLES).unwrap();
                assert!(s.cycles > SAMPLE_CYCLES - 32 && s.cycles <= SAMPLE_CYCLES);
            }
        }
    }

    #[test]
    fn calibration_scales_rate_startup_and_multi_group_allowance_with_the_machine() {
        let sample = Measurement {
            cycles: 5_000_000,
            loading_time: Duration::from_millis(2),
            execution_time: Duration::from_millis(10),
        };
        let fast = VmTiming::from_sample(sample).unwrap();
        let slow = VmTiming::from_sample(Measurement {
            loading_time: sample.loading_time * 4,
            execution_time: sample.execution_time * 4,
            ..sample
        })
        .unwrap();
        let cap = Duration::from_secs(8);
        assert_eq!(fast.cycles_per_ms, 31_250);
        assert_eq!(fast.per_group_load, Duration::from_millis(32));
        assert_eq!(fast.minimum, Duration::from_millis(192));
        assert_eq!(slow.minimum, fast.minimum * 4);
        let slow_loading = VmTiming::from_sample(Measurement {
            loading_time: Duration::from_millis(200),
            ..sample
        })
        .unwrap();
        assert_eq!(slow_loading.cycles_per_ms, fast.cycles_per_ms);
        assert_eq!(slow_loading.limit(1, cap), Duration::from_millis(3_360));
        for cycles in [0, 1, 100_000, 5_000_000, 50_000_000, u64::MAX] {
            assert!(slow.limit(cycles, cap) >= fast.limit(cycles, cap));
            assert!(slow.limit(cycles, cap) <= cap);
        }
        // Single-group additive startup (32 ms) + execution (1600 ms) = 1632 ms.
        assert_eq!(fast.limit(50_000_000, cap), Duration::from_millis(1_632));
        // Multi-group transactions receive proportional startup allowance, still capped.
        assert_eq!(
            fast.limit_for_groups(5_000_000, 10, cap),
            Duration::from_millis(480)
        );
        assert_eq!(fast.limit_for_groups(5_000_000, 10_000, cap), cap);
        assert_eq!(fast.limit(u64::MAX, cap), cap);
        assert_eq!(
            fast.limit(0, Duration::from_millis(1)),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn calibration_store_enforces_cooldown_single_flight_and_retains_valid_on_failure() {
        let store = CalibrationStore::new();
        let t0 = Instant::now();
        let initial = VmTiming::from_sample(Measurement {
            cycles: SAMPLE_CYCLES,
            loading_time: Duration::from_millis(1),
            execution_time: Duration::from_millis(10),
        })
        .unwrap();
        *store.entry.write() = Some(CalibrationEntry {
            timing: Some(initial),
            calibrated_at: t0,
        });

        // Fresh calibration refuses refresh.
        assert!(
            store
                .try_begin_refresh(Duration::from_secs(60), t0)
                .is_none()
        );

        // Stale calibration grants exactly one single-flight refresh guard.
        let t1 = t0 + Duration::from_secs(60);
        let guard = store
            .try_begin_refresh(Duration::from_secs(60), t1)
            .expect("stale calibration should start refresh");
        assert!(
            store
                .try_begin_refresh(Duration::from_secs(60), t1)
                .is_none(),
            "concurrent refresh must not stampede"
        );
        // Transient measurement failure preserves previous timing and advances cooldown.
        store.commit_refresh(None, t1);
        drop(guard);
        assert_eq!(store.entry.read().unwrap().timing, Some(initial));
        assert!(
            store
                .try_begin_refresh(Duration::from_secs(60), t1)
                .is_none()
        );

        // Subsequent stale refresh updates timing atomically.
        let t2 = t1 + Duration::from_secs(60);
        let slower = VmTiming::from_sample(Measurement {
            cycles: SAMPLE_CYCLES,
            loading_time: Duration::from_millis(2),
            execution_time: Duration::from_millis(20),
        })
        .unwrap();
        let guard = store
            .try_begin_refresh(Duration::from_secs(60), t2)
            .expect("cooldown elapsed");
        store.commit_refresh(Some(slower), t2);
        drop(guard);
        assert_eq!(store.entry.read().unwrap().timing, Some(slower));
    }

    #[test]
    fn zero_execution_cannot_define_a_rate_but_zero_loading_is_valid() {
        let sample = Measurement {
            cycles: SAMPLE_CYCLES,
            loading_time: Duration::ZERO,
            execution_time: Duration::ZERO,
        };
        for loading_time in [Duration::ZERO, Duration::from_millis(10)] {
            assert!(
                VmTiming::from_sample(Measurement {
                    loading_time,
                    ..sample
                })
                .is_none()
            );
        }
        let timing = VmTiming::from_sample(Measurement {
            execution_time: Duration::from_millis(10),
            ..sample
        })
        .unwrap();
        assert_eq!(timing.cycles_per_ms, 31_250);
        assert_eq!(timing.minimum, Duration::from_millis(160));
    }

    #[test]
    fn extreme_measurements_preserve_the_cap() {
        for elapsed in [Duration::from_nanos(1), Duration::MAX] {
            let timing = VmTiming::from_sample(Measurement {
                cycles: u64::MAX,
                loading_time: elapsed,
                execution_time: elapsed,
            })
            .unwrap();
            assert!(timing.cycles_per_ms > 0);
            assert!(!timing.minimum.is_zero());
            assert_eq!(timing.limit(u64::MAX, Duration::ZERO), Duration::ZERO);
            assert_eq!(
                timing.limit(u64::MAX, Duration::from_millis(1)),
                Duration::from_millis(1)
            );
        }
    }
}

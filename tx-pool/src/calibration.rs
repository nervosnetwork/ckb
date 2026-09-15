//! Machine calibration for network admission; never a consensus limit.

use ckb_script::{ScriptVersion, types::Machine};
use ckb_types::bytes::Bytes;
use ckb_vm::{
    DefaultMachineBuilder, DefaultMachineRunner, Error, SupportMachine,
    cost_model::estimate_cycles,
    elf::{LoadingAction, ProgramMetadata},
    memory::{FLAG_EXECUTABLE, FLAG_FREEZED},
};
use std::{
    num::NonZeroU128,
    sync::LazyLock,
    time::{Duration, Instant},
};

const SAMPLE_CYCLES: u64 = 5_000_000;

/// Machine-local measurements, without transaction-pool policy or peer input.
#[derive(Clone, Copy, Debug)]
struct Measurement {
    /// Executed consensus cycles in the fixed workload.
    cycles: u64,
    /// Time spent creating the machine and loading its program and stack.
    loading_time: Duration,
    /// Time spent running the loaded workload to its cycle limit.
    execution_time: Duration,
}

impl Measurement {
    /// Measure all supported script versions using the node's actual VM backend.
    ///
    /// Three samples per version suppress an isolated scheduling disturbance;
    /// retain the slowest version. The program has no syscalls or external data.
    /// Loading uses fixed metadata, so ELF parsing and data-provider I/O are not
    /// measured. Callers must leave room for those costs and workload variation.
    fn measure() -> Result<Self, Error> {
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
        let mut result = Self {
            cycles: SAMPLE_CYCLES,
            loading_time: Duration::ZERO,
            execution_time: Duration::ZERO,
        };
        for version in [ScriptVersion::V0, ScriptVersion::V1, ScriptVersion::V2] {
            let sample = || Self::sample(version, &program, &metadata, SAMPLE_CYCLES);
            let mut samples = [sample()?, sample()?, sample()?];
            samples.sort_unstable_by_key(|sample| sample.execution_time);
            let median = samples[1];
            if median.execution_time > result.execution_time {
                result.cycles = median.cycles;
                result.execution_time = median.execution_time;
            }
            samples.sort_unstable_by_key(|sample| sample.loading_time);
            result.loading_time = result.loading_time.max(samples[1].loading_time);
        }
        Ok(result)
    }

    fn sample(
        version: ScriptVersion,
        program: &Bytes,
        metadata: &ProgramMetadata,
        cycles: u64,
    ) -> Result<Self, Error> {
        let started = Instant::now();
        let core = version.init_core_machine(cycles);
        let mut machine = Machine::new(
            DefaultMachineBuilder::new(core)
                .instruction_cycle_func(Box::new(estimate_cycles))
                .build(),
        );
        machine.load_program_with_metadata(program, metadata, std::iter::empty())?;
        let loading_time = started.elapsed();
        let started = Instant::now();
        match machine.run() {
            Err(Error::CyclesExceeded) => Ok(Self {
                cycles: machine.machine().cycles(),
                loading_time,
                execution_time: started.elapsed(),
            }),
            Err(error) => Err(error),
            Ok(_) => Err(Error::Unexpected("calibration loop exited".into())),
        }
    }
}

static VM_TIMING: LazyLock<VmTiming> = LazyLock::new(|| {
    let timing = match Measurement::measure() {
        Ok(sample) => VmTiming::from_sample(sample),
        Err(error) => {
            ckb_logger::warn!("VM calibration failed; using the network time cap: {error}");
            return VmTiming {
                cycles_per_ms: 1,
                minimum: Duration::MAX,
            };
        }
    };
    ckb_logger::info!(
        "Network VM calibration: {} cycles/ms, minimum {} ms",
        timing.cycles_per_ms,
        timing.minimum.as_millis(),
    );
    timing
});

/// Calibrate before the pool accepts work; reuse the process-wide result.
pub(crate) fn initialize() {
    LazyLock::force(&VM_TIMING);
}

pub(crate) fn for_cycles(cycles: u64, cap: Duration) -> Duration {
    VM_TIMING.limit(cycles, cap)
}

struct VmTiming {
    cycles_per_ms: u64,
    minimum: Duration,
}

impl VmTiming {
    fn from_sample(sample: Measurement) -> Self {
        // A small, cache-hot loop is much cheaper than arbitrary scripts and
        // their providers. Give both measurements the same conservative slack.
        // The minimum admits one whole calibration quantum (load + execution),
        // even for tiny declarations; it scales with this machine as well.
        const MARGIN: u32 = 16;
        let nanos = sample
            .execution_time
            .as_nanos()
            .saturating_mul(MARGIN.into());
        let nanos = NonZeroU128::new(nanos).unwrap_or(NonZeroU128::MIN);
        let cycles_per_ms =
            (u128::from(sample.cycles) * 1_000_000 / nanos).clamp(1, u64::MAX.into()) as u64;
        let minimum = sample
            .loading_time
            .saturating_add(sample.execution_time)
            .saturating_mul(MARGIN);
        let millis = minimum
            .as_nanos()
            .div_ceil(1_000_000)
            .clamp(1, u64::MAX.into());
        Self {
            cycles_per_ms,
            minimum: Duration::from_millis(millis as u64),
        }
    }

    fn limit(&self, cycles: u64, cap: Duration) -> Duration {
        Duration::from_millis(cycles.div_ceil(self.cycles_per_ms))
            .max(self.minimum)
            .min(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_workload_runs_to_its_cycle_limit_on_every_script_version() {
        let sample = Measurement::measure().unwrap();
        assert!(sample.cycles > SAMPLE_CYCLES - 32);
        assert!(sample.cycles <= SAMPLE_CYCLES);
        assert!(!sample.loading_time.is_zero());
        assert!(!sample.execution_time.is_zero());
    }

    #[test]
    fn calibration_scales_rate_and_startup_allowance_with_the_machine() {
        let sample = Measurement {
            cycles: 5_000_000,
            loading_time: Duration::from_millis(2),
            execution_time: Duration::from_millis(10),
        };
        let fast = VmTiming::from_sample(sample);
        let slow = VmTiming::from_sample(Measurement {
            loading_time: sample.loading_time * 4,
            execution_time: sample.execution_time * 4,
            ..sample
        });
        let cap = Duration::from_secs(8);
        assert_eq!(fast.cycles_per_ms, 31_250);
        assert_eq!(fast.minimum, Duration::from_millis(192));
        assert_eq!(slow.minimum, fast.minimum * 4);
        let slow_loading = VmTiming::from_sample(Measurement {
            loading_time: Duration::from_millis(200),
            ..sample
        });
        assert_eq!(slow_loading.cycles_per_ms, fast.cycles_per_ms);
        assert_eq!(slow_loading.limit(1, cap), Duration::from_millis(3_360));
        for cycles in [0, 1, 100_000, 5_000_000, 50_000_000, u64::MAX] {
            assert!(slow.limit(cycles, cap) >= fast.limit(cycles, cap));
            assert!(slow.limit(cycles, cap) <= cap);
        }
        assert_eq!(fast.limit(6_000_001, cap), Duration::from_millis(193));
        assert_eq!(fast.limit(50_000_000, cap), Duration::from_millis(1_600));
        assert_eq!(fast.limit(u64::MAX, cap), cap);
        assert_eq!(
            fast.limit(0, Duration::from_millis(1)),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn coarse_clocks_and_extreme_measurements_preserve_the_cap() {
        for elapsed in [Duration::ZERO, Duration::from_nanos(1), Duration::MAX] {
            let timing = VmTiming::from_sample(Measurement {
                cycles: u64::MAX,
                loading_time: elapsed,
                execution_time: elapsed,
            });
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

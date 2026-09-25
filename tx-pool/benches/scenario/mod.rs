//! Parse fixture shape and submission policy once, before allocating a workload.

use ckb_types::core::tx_pool::TRANSACTION_SIZE_LIMIT;

pub(crate) const MAX_TRANSACTIONS: usize = 65_536;
pub(crate) const FANOUT_COHORT_SIZE: usize = 65;
// Fixed Molecule sizes for build_success_tx: one dep, empty witnesses/data,
// 44-byte inputs and 89 bytes per output (including both vector offsets).
pub(crate) const INPUT_BYTES: usize = 44;
pub(crate) const OUTPUT_BYTES: usize = 89;
pub(crate) const FANIN_BASE_BYTES: usize = 198;
pub(crate) const FANOUT_BASE_BYTES: usize = 153;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Workload {
    AlwaysSuccess,
    FanIn(usize),
    Secp256k1,
    Chain { reverse: bool },
    Forest { depth: usize, reverse: bool },
    Fanout { reverse: bool },
    FanoutCohorts,
    RbfPairs { windowed: bool },
}

impl Workload {
    /// Concurrent chains may resolve children before their parents are accepted,
    /// even when their submitted vector is in forward order.
    pub(crate) fn permits_unknown_parents(self) -> bool {
        match self {
            Self::Chain { .. }
            | Self::Forest { reverse: true, .. }
            | Self::Fanout { reverse: true }
            | Self::FanoutCohorts => true,
            Self::AlwaysSuccess
            | Self::FanIn(_)
            | Self::Secp256k1
            | Self::Forest { reverse: false, .. }
            | Self::Fanout { reverse: false }
            | Self::RbfPairs { .. } => false,
        }
    }

    pub(crate) fn is_reverse(self) -> bool {
        matches!(
            self,
            Self::Chain { reverse: true }
                | Self::Forest { reverse: true, .. }
                | Self::Fanout { reverse: true }
                | Self::FanoutCohorts
        )
    }

    pub(crate) fn is_rbf_pairs(self) -> bool {
        matches!(self, Self::RbfPairs { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubmissionOrder {
    Concurrent,
    WindowedRbf,
    Forest(usize),
    ParentFirst,
    ReverseFanoutCohorts,
}

/// The name is the reported experiment; workload is the parsed fixture shared
/// by construction, preflight verification and submission policy.
pub(crate) struct BenchmarkScenario {
    pub(crate) name: String,
    pub(crate) workload: Workload,
    pub(crate) target_count: usize,
    pub(crate) warm_count: usize,
    pub(crate) workers: usize,
    pub(crate) peers: usize,
    pub(crate) callback_delay_us: Option<u64>,
    pub(crate) reorg_in_flight: bool,
}

impl BenchmarkScenario {
    pub(crate) fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let name = args.next().unwrap_or_else(|| "always_success".to_owned());
        let mut number = |default| match args.next() {
            Some(value) => value.parse::<usize>().map_err(|error| error.to_string()),
            None => Ok(default),
        };
        let target_count = number(1_000)?;
        let warm_count = number(100)?;
        let workers = number(8)?;
        let peers = number(8)?;
        require(
            target_count != 0 && workers != 0 && peers != 0,
            "target, workers and peers must be non-zero",
        )?;
        let population = target_count
            .checked_add(warm_count)
            .filter(|&count| count <= MAX_TRANSACTIONS)
            .ok_or("target + warm must be at most 65536")?;
        let callback_delay_us = name
            .strip_prefix("always_success_callback_")
            .and_then(|value| value.strip_suffix("us"))
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|error| error.to_string())?
            .or_else(|| (name == "reorg_in_flight").then_some(500));
        let reorg_in_flight = name == "reorg_in_flight";
        let workload = match name.as_str() {
            "always_success" | "reorg_in_flight" => Workload::AlwaysSuccess,
            "secp256k1" => Workload::Secp256k1,
            "dependent" => Workload::Chain { reverse: false },
            "dependent_reverse" => Workload::Chain { reverse: true },
            "fanout" => Workload::Fanout { reverse: false },
            "fanout_reverse" => Workload::Fanout { reverse: true },
            "fanout_ready_64_reverse" => Workload::FanoutCohorts,
            "rbf_pairs" | "rbf_pressure" => Workload::RbfPairs { windowed: false },
            "rbf_pairs_windowed" => Workload::RbfPairs { windowed: true },
            _ if callback_delay_us.is_some() => Workload::AlwaysSuccess,
            _ => {
                if let Some(spec) = name.strip_prefix("dependent_forest_") {
                    let (depth, reverse) = spec
                        .strip_suffix("_reverse")
                        .map_or((spec, false), |depth| (depth, true));
                    let depth = depth.parse::<usize>().map_err(|error| error.to_string())?;
                    require(depth != 0, "dependency depth must be non-zero")?;
                    Workload::Forest { depth, reverse }
                } else if let Some(spec) = name.strip_prefix("always_success_fanin_") {
                    let fan_in = spec.parse::<usize>().map_err(|error| error.to_string())?;
                    require(fan_in != 0, "fan-in must be non-zero")?;
                    Workload::FanIn(fan_in)
                } else {
                    return Err(format!("unknown scenario: {name}"));
                }
            }
        };
        require(
            !workload.is_rbf_pairs() || warm_count == target_count,
            "RBF workload requires equal warm and target counts",
        )?;
        require(
            !workload.is_reverse() || workload == Workload::FanoutCohorts || warm_count == 0,
            "reverse dependency workloads require warm=0",
        )?;
        match workload {
            Workload::Forest {
                depth,
                reverse: false,
            } => {
                require(
                    target_count.is_multiple_of(depth) && warm_count.is_multiple_of(depth),
                    "forward forest target and warm must each contain complete chains",
                )?;
            }
            Workload::FanoutCohorts => {
                require(
                    target_count.is_multiple_of(FANOUT_COHORT_SIZE)
                        && warm_count.is_multiple_of(FANOUT_COHORT_SIZE),
                    "fanout cohort target and warm counts must each be multiples of 65",
                )?;
            }
            Workload::Fanout { .. } => {
                require(population >= 2, "fanout requires a parent and a child")?;
                require(
                    FANOUT_BASE_BYTES + OUTPUT_BYTES * (population - 1)
                        <= TRANSACTION_SIZE_LIMIT as usize,
                    "fanout parent exceeds transaction size limit",
                )?;
            }
            Workload::FanIn(fan_in) => {
                require(
                    fan_in <= (TRANSACTION_SIZE_LIMIT as usize - FANIN_BASE_BYTES) / INPUT_BYTES,
                    "fan-in transaction exceeds transaction size limit",
                )?;
            }
            _ => {}
        }
        Ok(Self {
            name,
            workload,
            target_count,
            warm_count,
            workers,
            peers,
            callback_delay_us,
            reorg_in_flight,
        })
    }

    pub(crate) fn submission_orders(&self) -> (SubmissionOrder, SubmissionOrder) {
        let order = match self.workload {
            Workload::Forest {
                depth,
                reverse: false,
            } => SubmissionOrder::Forest(depth),
            Workload::RbfPairs { windowed: true } => SubmissionOrder::WindowedRbf,
            Workload::FanoutCohorts => SubmissionOrder::ReverseFanoutCohorts,
            _ => SubmissionOrder::Concurrent,
        };
        if self.workload == (Workload::Fanout { reverse: false }) {
            if self.warm_count == 0 {
                (order, SubmissionOrder::ParentFirst)
            } else {
                (SubmissionOrder::ParentFirst, order)
            }
        } else {
            (order, order)
        }
    }
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_boundary_cases_are_checked_before_fixture_allocation() {
        use super::BenchmarkScenario;
        let cases: serde_json::Value = serde_json::from_str(include_str!("cases.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let args = case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned());
            assert_eq!(
                BenchmarkScenario::parse(args).is_ok(),
                case["valid"].as_bool().unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn parsed_shape_drives_fixture_and_submission_policy() {
        use super::{BenchmarkScenario, SubmissionOrder, Workload};

        let parse = |name: &str, warm: &str| {
            BenchmarkScenario::parse([name, "130", warm, "8", "4"].map(str::to_owned).into_iter())
                .unwrap()
        };
        let forest = parse("dependent_forest_10", "10");
        assert_eq!(
            (
                &forest.name[..],
                forest.target_count,
                forest.warm_count,
                forest.workers,
                forest.peers
            ),
            ("dependent_forest_10", 130, 10, 8, 4)
        );
        assert!(!forest.reorg_in_flight);
        let reorg = parse("reorg_in_flight", "0");
        assert!(reorg.reorg_in_flight);
        assert_eq!(reorg.callback_delay_us, Some(500));
        assert_eq!(
            forest.workload,
            Workload::Forest {
                depth: 10,
                reverse: false
            }
        );
        assert_eq!(
            forest.submission_orders(),
            (SubmissionOrder::Forest(10), SubmissionOrder::Forest(10))
        );
        for name in ["dependent", "dependent_reverse"] {
            assert_eq!(
                parse(name, "0").submission_orders(),
                (SubmissionOrder::Concurrent, SubmissionOrder::Concurrent)
            );
        }
        assert_eq!(
            parse("fanout", "0").submission_orders(),
            (SubmissionOrder::Concurrent, SubmissionOrder::ParentFirst)
        );
        assert_eq!(
            parse("fanout", "1").submission_orders(),
            (SubmissionOrder::ParentFirst, SubmissionOrder::Concurrent)
        );
        assert_eq!(
            parse("fanout_ready_64_reverse", "65").submission_orders(),
            (
                SubmissionOrder::ReverseFanoutCohorts,
                SubmissionOrder::ReverseFanoutCohorts
            )
        );
        assert_eq!(
            parse("rbf_pairs_windowed", "130").submission_orders(),
            (SubmissionOrder::WindowedRbf, SubmissionOrder::WindowedRbf)
        );
        let callback = parse("always_success_callback_0us", "0");
        assert_eq!(callback.workload, Workload::AlwaysSuccess);
        assert_eq!(callback.callback_delay_us, Some(0));
    }

    #[test]
    fn missing_parent_permission_follows_readiness_not_vector_direction() {
        use super::BenchmarkScenario;

        for (name, target, warm, permitted) in [
            ("dependent", "8", "2", true),
            ("dependent_reverse", "8", "0", true),
            ("dependent_forest_4_reverse", "8", "0", true),
            ("fanout_reverse", "8", "0", true),
            ("fanout_ready_64_reverse", "130", "65", true),
            ("dependent_forest_4", "8", "4", false),
            ("fanout", "8", "0", false),
            ("fanout", "8", "2", false),
            ("always_success", "8", "0", false),
            ("always_success_fanin_2", "8", "0", false),
            ("always_success_callback_500us", "8", "0", false),
            ("secp256k1", "8", "0", false),
            ("rbf_pairs", "8", "8", false),
            ("rbf_pairs_windowed", "8", "8", false),
            ("rbf_pressure", "8", "8", false),
            ("reorg_in_flight", "8", "0", false),
        ] {
            let scenario = BenchmarkScenario::parse(
                [name, target, warm, "2", "2"]
                    .map(str::to_owned)
                    .into_iter(),
            )
            .unwrap();
            assert_eq!(
                scenario.workload.permits_unknown_parents(),
                permitted,
                "{name}"
            );
        }
    }
}

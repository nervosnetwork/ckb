//! Shared corpus and result contract for the current and develop executors.
//! Fees/cycles are controlled accepted metadata: this does not run admission,
//! script verification, DAO calculation, or final block serialization.

use crate::allocation_observation::{begin_allocation_window, end_allocation_window};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_hash::blake2b_256;
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_tx_pool::{
    TxEntry,
    packing_bench::{ADAPTER, PackingSource},
};
use ckb_types::{
    H256, U256,
    bytes::Bytes,
    core::{
        Capacity, TransactionBuilder,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    hint::black_box,
    sync::Arc,
    time::Instant,
};

const CONTRACT: &str = "template_selection_v2";
const MAX_ANCESTORS: usize = 64;
const MAX_ENTRIES: usize = 16_384;

fn require(ok: bool, message: &str) -> Result<(), String> {
    ok.then_some(()).ok_or_else(|| message.into())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", H256::from(blake2b_256(bytes)))
}

fn explicit_limits(value: &str) -> Option<(usize, u64)> {
    let mut fields = value.split(':');
    if fields.next()? != "budget" {
        return None;
    }
    let bytes = fields.next()?;
    let cycles = fields.next()?;
    if !bytes.bytes().all(|byte| byte.is_ascii_digit())
        || !cycles.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let bytes = bytes.parse().ok()?;
    let cycles = cycles.parse().ok()?;
    fields.next().is_none().then_some((bytes, cycles))
}

#[derive(Clone, Debug)]
struct Parameters {
    shape: String,
    fees: String,
    count: usize,
    limit: String,
    repeats: usize,
    warm: usize,
}

impl Parameters {
    fn parse(args: &[String]) -> Result<Self, String> {
        require(
            args.len() == 6,
            "usage: packing_one_shot SHAPE FEES COUNT LIMIT REPEATS WARM",
        )?;
        let value = Self {
            shape: args[0].clone(),
            fees: args[1].clone(),
            count: args[2].parse::<usize>().map_err(|e| e.to_string())?,
            limit: args[3].clone(),
            repeats: args[4].parse::<usize>().map_err(|e| e.to_string())?,
            warm: args[5].parse::<usize>().map_err(|e| e.to_string())?,
        };
        require(
            ["independent", "chain", "fanout", "diamond", "mixed"].contains(&value.shape.as_str()),
            "unknown shape",
        )?;
        require(
            ["equal", "varied", "cpfp"].contains(&value.fees.as_str()),
            "unknown fee distribution",
        )?;
        require(
            ["all", "bytes", "cycles", "both", "zero"].contains(&value.limit.as_str())
                || explicit_limits(&value.limit).is_some(),
            "unknown capacity regime; use all/bytes/cycles/both/zero or budget:BYTES:CYCLES",
        )?;
        require(
            (1..=MAX_ENTRIES).contains(&value.count),
            "count must be in 1..=16384",
        )?;
        require(
            (1..=128).contains(&value.repeats) && value.warm <= 32,
            "repeats must be in 1..=128 and warm <= 32",
        )?;
        Ok(value)
    }

    fn scenario(&self) -> String {
        format!(
            "packing_{}_{}_{}_{}",
            self.shape, self.fees, self.count, self.limit
        )
    }
}

/// Causal input and read-only cell-dep edges. No fixture spends a dep that any
/// other fixture reads, so develop's historical eviction policy admits it too.
type FixtureEdges = (Vec<(usize, u32)>, Vec<(usize, u32)>, usize);

fn edges(shape: &str, i: usize) -> FixtureEdges {
    let (width, offset) = match shape {
        "chain" => (64, i % 64),
        "fanout" => (65, i % 65),
        "diamond" => (4, i % 4),
        "mixed" => (8, i % 8),
        _ => (1, 0),
    };
    let base = i - offset;
    match shape {
        "chain" if offset != 0 => (vec![(i - 1, 0)], vec![], 2),
        "fanout" if offset != 0 => (vec![(base, (offset - 1) as u32)], vec![], 2),
        "fanout" => (vec![], vec![], width - 1),
        "diamond" => match offset {
            1 => (vec![(base, 0)], vec![], 2),
            2 => (vec![(base, 1)], vec![], 2),
            3 => (vec![(base + 1, 0), (base + 2, 0)], vec![], 2),
            _ => (vec![], vec![], 2),
        },
        "mixed" => match offset {
            1 => (vec![(base, 0)], vec![], 2),
            2 => (vec![], vec![(base, 1)], 2),
            3 => (vec![(base + 1, 0), (base + 2, 0)], vec![], 2),
            5 => (vec![(base + 4, 0)], vec![], 2),
            6 => (vec![(base + 5, 0)], vec![(base + 3, 0)], 2),
            7 => (vec![], vec![(base + 2, 1), (base + 6, 0)], 2),
            _ => (vec![], vec![], 2),
        },
        _ => (vec![], vec![], 2),
    }
}

struct Fixture {
    entries: Vec<TxEntry>,
    parents: Vec<BTreeSet<usize>>,
    indexes: BTreeMap<Byte32, usize>,
    snapshot: Arc<Snapshot>,
    _store: MockStore,
    total_bytes: usize,
    total_cycles: u64,
    total_fees: u64,
    dep_edges: usize,
    digest: String,
}

impl Fixture {
    fn new(parameters: &Parameters) -> Self {
        let graph: Vec<_> = (0..parameters.count)
            .map(|i| edges(&parameters.shape, i))
            .collect();
        let parents: Vec<BTreeSet<_>> = graph
            .iter()
            .map(|(inputs, deps, _)| {
                inputs
                    .iter()
                    .chain(deps)
                    .map(|(parent, _)| *parent)
                    .collect()
            })
            .collect();
        let has_children: BTreeSet<_> = parents.iter().flatten().copied().collect();
        let mut entries: Vec<TxEntry> = Vec::with_capacity(parameters.count);
        let mut identity = Vec::new();
        let mut dep_edges = 0;
        for (i, (input_edges, dep_relations, outputs)) in graph.iter().enumerate() {
            let input_points: Vec<_> = if input_edges.is_empty() {
                vec![OutPoint::new(
                    Byte32::from_slice(&[0xee; 32]).expect("32 bytes"),
                    i as u32,
                )]
            } else {
                input_edges
                    .iter()
                    .map(|(parent, output)| {
                        OutPoint::new(entries[*parent].transaction().hash(), *output)
                    })
                    .collect()
            };
            let dep_points: Vec<_> = dep_relations
                .iter()
                .map(|(parent, output)| {
                    OutPoint::new(entries[*parent].transaction().hash(), *output)
                })
                .collect();
            dep_edges += dep_points.len();
            let tx = TransactionBuilder::default()
                .inputs(
                    input_points
                        .iter()
                        .map(|point| CellInput::new(point.clone(), 0)),
                )
                .cell_deps(
                    dep_points
                        .iter()
                        .map(|point| CellDep::new_builder().out_point(point.clone()).build()),
                )
                .outputs((0..*outputs).map(|_| CellOutput::default()))
                .outputs_data(
                    (0..*outputs).map(|_| Bytes::copy_from_slice(&(i as u64).to_le_bytes()).pack()),
                )
                .build();
            let size = tx.data().serialized_size_in_block();
            // All cycles are below the byte-weight crossover. Equal distribution
            // therefore gives equal own and package fee rates, testing arrival ties.
            let cycles = size as u64 * (100 + (i as u64 % 5) * 100);
            let rate = match parameters.fees.as_str() {
                "varied" => 1_000 * (1 + (i as u64 * 17 % 13)),
                "cpfp" if has_children.contains(&i) => 10,
                "cpfp" if !parents[i].is_empty() => 100_000,
                _ => 1_000,
            };
            let fee = size as u64 * rate;
            let timestamp = i as u64 + 1;
            identity.extend_from_slice(tx.data().as_slice());
            for number in [size as u64, cycles, fee, timestamp] {
                identity.extend_from_slice(&number.to_le_bytes());
            }
            let metadata = |out_point| CellMeta {
                out_point,
                ..CellMeta::default()
            };
            let rtx = ResolvedTransaction {
                transaction: tx,
                resolved_inputs: input_points.into_iter().map(metadata).collect(),
                resolved_cell_deps: dep_points.into_iter().map(metadata).collect(),
                resolved_dep_groups: Vec::new(),
            };
            entries.push(TxEntry::new_with_timestamp(
                Arc::new(rtx),
                cycles,
                Capacity::shannons(fee),
                size,
                timestamp,
            ));
        }
        let store = MockStore::default();
        let consensus = Arc::new(ConsensusBuilder::default().build());
        // Packing consults ProposalView only. No fake forced_status or DB reads.
        let snapshot = Arc::new(Snapshot::new(
            consensus.genesis_block().header(),
            U256::zero(),
            consensus.genesis_epoch_ext().clone(),
            store.store().get_snapshot(),
            ProposalView::new(
                HashSet::new(),
                entries
                    .iter()
                    .map(TxEntry::proposal_short_id)
                    .collect::<HashSet<_>>(),
            ),
            consensus,
        ));
        Self {
            indexes: entries
                .iter()
                .enumerate()
                .map(|(i, entry)| (entry.transaction().hash(), i))
                .collect(),
            total_bytes: entries.iter().map(|entry| entry.size).sum(),
            total_cycles: entries.iter().map(|entry| entry.cycles).sum(),
            total_fees: entries.iter().map(|entry| entry.fee.as_u64()).sum(),
            entries,
            parents,
            snapshot,
            _store: store,
            dep_edges,
            digest: digest(&identity),
        }
    }

    fn limits(&self, mode: &str) -> (usize, u64) {
        if let Some(limits) = explicit_limits(mode) {
            return limits;
        }
        match mode {
            "bytes" => (self.total_bytes / 3, self.total_cycles),
            "cycles" => (self.total_bytes, self.total_cycles / 3),
            "both" => (self.total_bytes * 2 / 3, self.total_cycles * 2 / 3),
            "zero" => (0, 0),
            _ => (self.total_bytes, self.total_cycles),
        }
    }

    fn validate(&self, selected: &[TxEntry], limits: (usize, u64)) -> Result<Value, String> {
        let mut seen = BTreeSet::new();
        let (mut bytes, mut cycles, mut fees) = (0usize, 0u64, 0u64);
        let mut order = Vec::new();
        let mut hashes = Vec::new();
        for entry in selected {
            let hash = entry.transaction().hash();
            let i = *self
                .indexes
                .get(&hash)
                .ok_or("selected unknown transaction")?;
            require(!seen.contains(&i), "selected duplicate transaction")?;
            require(
                self.parents[i].iter().all(|parent| seen.contains(parent)),
                "selected dependency missing or out of order",
            )?;
            let expected = &self.entries[i];
            require(
                entry.size == expected.size
                    && entry.cycles == expected.cycles
                    && entry.fee == expected.fee,
                "selected accounting differs from fixture",
            )?;
            require(
                entry.transaction().data().as_slice() == expected.transaction().data().as_slice(),
                "selected transaction changed",
            )?;
            seen.insert(i);
            bytes = bytes
                .checked_add(entry.size)
                .ok_or("selected size overflow")?;
            cycles = cycles
                .checked_add(entry.cycles)
                .ok_or("selected cycles overflow")?;
            fees = fees
                .checked_add(entry.fee.as_u64())
                .ok_or("selected fee overflow")?;
            order.extend_from_slice(hash.as_slice());
            hashes.push(hash);
        }
        require(
            bytes <= limits.0 && cycles <= limits.1,
            "selected result exceeds capacity",
        )?;
        hashes.sort_unstable();
        let sorted: Vec<_> = hashes
            .iter()
            .flat_map(|hash| hash.as_slice().iter().copied())
            .collect();
        Ok(json!({
            "selected_tx": selected.len(), "selected_bytes": bytes, "selected_cycles": cycles, "selected_fees": fees,
            "ordered_digest": digest(&order), "set_digest": digest(&sorted),
            "byte_utilization": if limits.0 == 0 { 0.0 } else { bytes as f64 / limits.0 as f64 },
            "cycle_utilization": if limits.1 == 0 { 0.0 } else { cycles as f64 / limits.1 as f64 },
        }))
    }

    /// Exhaustive fee-quality oracle for small fixtures only. Selection is a
    /// greedy algorithm, so lower fee than this optimum is an observation.
    fn optimum(&self, limits: (usize, u64)) -> Option<u64> {
        if self.entries.len() > 16 {
            return None;
        }
        let mut best = 0;
        for mask in 0u64..(1u64 << self.entries.len()) {
            let (mut bytes, mut cycles, mut fees) = (0, 0, 0);
            let mut closed = true;
            for (i, entry) in self.entries.iter().enumerate() {
                if mask & (1 << i) == 0 {
                    continue;
                }
                closed &= self.parents[i]
                    .iter()
                    .all(|parent| mask & (1 << parent) != 0);
                bytes += entry.size;
                cycles += entry.cycles;
                fees += entry.fee.as_u64();
            }
            if closed && bytes <= limits.0 && cycles <= limits.1 {
                best = best.max(fees);
            }
        }
        Some(best)
    }
}

// Match the admission observer's cumulative request semantics. JSON creation
// happens after the counter is disabled and never contributes to this window.
fn allocation_record((calls, requested_bytes): (u64, u64)) -> Value {
    if cfg!(feature = "allocation-observation") {
        json!({"calls": calls, "requested_bytes": requested_bytes})
    } else {
        Value::Null
    }
}

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let parameters = Parameters::parse(args)?;
    begin_allocation_window();
    let fixture_started = Instant::now();
    let fixture = Fixture::new(&parameters);
    let fixture_ns = fixture_started.elapsed().as_nanos();
    let fixture_allocation = allocation_record(end_allocation_window());
    begin_allocation_window();
    let setup_started = Instant::now();
    let source = PackingSource::new(
        &fixture.entries,
        Arc::clone(&fixture.snapshot),
        MAX_ANCESTORS,
    );
    let setup_ns = setup_started.elapsed().as_nanos();
    let setup_allocation = allocation_record(end_allocation_window());
    let source = source?;
    let limits = fixture.limits(&parameters.limit);
    let all = source.select(fixture.total_bytes, fixture.total_cycles)?;
    fixture.validate(&all, (fixture.total_bytes, fixture.total_cycles))?;
    require(
        all.len() == parameters.count,
        "fixture is not completely packable at full capacity",
    )?;
    drop(all);
    let reference = fixture.validate(&source.select(limits.0, limits.1)?, limits)?;
    let optimal_fee = fixture.optimum(limits);
    for _ in 0..parameters.warm {
        let result = source.select(limits.0, limits.1)?;
        fixture.validate(&result, limits)?;
    }
    println!(
        "PACKING_BUILD {}",
        json!({
            "schema_version": 1, "contract": CONTRACT, "adapter": ADAPTER,
            "debug_assertions": cfg!(debug_assertions), "packing_bench": cfg!(feature = "packing-bench"),
            "profiling": cfg!(feature = "profiling"), "allocation_observation": cfg!(feature = "allocation-observation"),
            "tokio_trace": cfg!(feature = "tokio-trace"),
        })
    );
    println!(
        "PACKING_CORPUS {}",
        json!({
            "schema_version": 1, "scenario": parameters.scenario(), "shape": parameters.shape, "fees": parameters.fees,
            "count": parameters.count, "limit": parameters.limit, "repeats": parameters.repeats, "warm": parameters.warm,
            "fixture_digest": fixture.digest, "fixture_ns": fixture_ns, "setup_ns": setup_ns,
            "fixture_allocation": fixture_allocation, "setup_allocation": setup_allocation,
            "retained_source_entries": fixture.entries.len(), "causal_edges": fixture.parents.iter().map(BTreeSet::len).sum::<usize>(),
            "cell_dep_edges": fixture.dep_edges, "max_ancestors": MAX_ANCESTORS,
            "total_bytes": fixture.total_bytes, "total_cycles": fixture.total_cycles, "total_fees": fixture.total_fees,
            "bytes_limit": limits.0, "cycles_limit": limits.1, "optimal_fee_small_fixture": optimal_fee,
            "reference_result": reference,
            "scope": "accepted transactions only; no capture locks, VM, DAO, proposals/uncles packing, or final block serialization",
        })
    );
    for repeat in 0..parameters.repeats {
        let anchor = crate::measurement_clock::ClockAnchor::capture()?;
        begin_allocation_window();
        let started = Instant::now();
        let selected = black_box(source.select(black_box(limits.0), black_box(limits.1)));
        let ended = Instant::now();
        let allocation = allocation_record(end_allocation_window());
        let endpoint = crate::measurement_clock::ClockAnchor::capture()?;
        let selected = selected?;
        let result = fixture.validate(&selected, limits)?;
        let set_deterministic = result["set_digest"] == reference["set_digest"];
        let ordered_deterministic = result["ordered_digest"] == reference["ordered_digest"];
        if ADAPTER == "authority_selection_v1" {
            require(
                ordered_deterministic && set_deterministic,
                "current selection changed across identical calls",
            )?;
        }
        let window = anchor.window(&parameters.scenario(), started, ended, &endpoint)?;
        println!(
            "PACKING_SAMPLE {}",
            json!({
                "schema_version": 1, "repeat": repeat, "window": window, "result": result,
                "ordered_deterministic": ordered_deterministic, "set_deterministic": set_deterministic,
                "allocation": allocation,
            })
        );
        // Validation, serialization, and the returned Vec<TxEntry> destruction
        // are outside timing. Selection's own temporary state drops inside it.
        drop(selected);
    }
    println!(
        "PACKING_COMPLETE {}",
        json!({"schema_version": 1, "samples": parameters.repeats})
    );
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn finite_executor_reaches_validated_completion() {
        use super::*;
        run(&["mixed", "cpfp", "8", "both", "2", "1"].map(str::to_owned)).unwrap();
    }

    #[test]
    fn production_matrix_checks_dependencies_limits_and_repeatability() {
        use super::*;
        for shape in ["independent", "chain", "fanout", "diamond", "mixed"] {
            for fees in ["equal", "varied", "cpfp"] {
                let p = Parameters {
                    shape: shape.into(),
                    fees: fees.into(),
                    count: 8,
                    limit: "all".into(),
                    repeats: 2,
                    warm: 0,
                };
                let fixture = Fixture::new(&p);
                let source = PackingSource::new(
                    &fixture.entries,
                    Arc::clone(&fixture.snapshot),
                    MAX_ANCESTORS,
                )
                .unwrap();
                for mode in [
                    "all",
                    "bytes",
                    "cycles",
                    "both",
                    "zero",
                    "budget:1000:100000",
                ] {
                    let limits = fixture.limits(mode);
                    let first = source.select(limits.0, limits.1).unwrap();
                    let observed = fixture.validate(&first, limits).unwrap();
                    let second = fixture
                        .validate(&source.select(limits.0, limits.1).unwrap(), limits)
                        .unwrap();
                    if ADAPTER == "authority_selection_v1" {
                        assert_eq!(observed, second);
                    }
                    assert!(
                        observed["selected_fees"].as_u64().unwrap()
                            <= fixture.optimum(limits).unwrap()
                    );
                    if mode == "all" {
                        assert_eq!(first.len(), p.count);
                    }
                    if mode == "zero" {
                        assert!(first.is_empty());
                    }
                    if mode == "bytes" || mode == "cycles" {
                        assert!(first.len() < p.count);
                    }
                }
            }
        }
    }

    #[test]
    fn validator_rejects_missing_dependencies_duplicates_and_capacity_overflow() {
        use super::*;
        let p = Parameters {
            shape: "chain".into(),
            fees: "equal".into(),
            count: 3,
            limit: "all".into(),
            repeats: 1,
            warm: 0,
        };
        let fixture = Fixture::new(&p);
        let limits = fixture.limits("all");
        assert!(fixture.validate(&fixture.entries[1..2], limits).is_err());
        assert!(
            fixture
                .validate(
                    &[fixture.entries[0].clone(), fixture.entries[0].clone()],
                    limits
                )
                .is_err()
        );
        assert!(fixture.validate(&fixture.entries[..1], (0, 0)).is_err());
        let mut altered = fixture.entries[0].clone();
        altered.cycles += 1;
        assert!(fixture.validate(&[altered], limits).is_err());
    }

    #[test]
    fn equal_fee_independent_entries_follow_arrival_ties_in_current_selection() {
        use super::*;
        if ADAPTER != "authority_selection_v1" {
            return;
        }
        let p = Parameters {
            shape: "independent".into(),
            fees: "equal".into(),
            count: 8,
            limit: "bytes".into(),
            repeats: 1,
            warm: 0,
        };
        let fixture = Fixture::new(&p);
        let source = PackingSource::new(
            &fixture.entries,
            Arc::clone(&fixture.snapshot),
            MAX_ANCESTORS,
        )
        .unwrap();
        let limits = fixture.limits("bytes");
        let selected = source.select(limits.0, limits.1).unwrap();
        assert!(!selected.is_empty() && selected.len() < fixture.entries.len());
        assert_eq!(
            selected
                .iter()
                .map(TxEntry::proposal_short_id)
                .collect::<Vec<_>>(),
            fixture.entries[..selected.len()]
                .iter()
                .map(TxEntry::proposal_short_id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cpfp_child_pulls_its_package_ahead_of_an_independent_competitor() {
        use super::*;
        let p = Parameters {
            shape: "mixed".into(),
            fees: "cpfp".into(),
            count: 5,
            limit: "bytes".into(),
            repeats: 1,
            warm: 0,
        };
        let fixture = Fixture::new(&p);
        let bytes: usize = fixture.entries[..4].iter().map(|entry| entry.size).sum();
        let source = PackingSource::new(
            &fixture.entries,
            Arc::clone(&fixture.snapshot),
            MAX_ANCESTORS,
        )
        .unwrap();
        let selected = source.select(bytes, fixture.total_cycles).unwrap();
        fixture
            .validate(&selected, (bytes, fixture.total_cycles))
            .unwrap();
        assert_eq!(selected.len(), 4);
        assert_eq!(
            selected
                .iter()
                .map(TxEntry::proposal_short_id)
                .collect::<BTreeSet<_>>(),
            fixture.entries[..4]
                .iter()
                .map(TxEntry::proposal_short_id)
                .collect()
        );
    }

    #[test]
    fn partial_fanout_executes_with_validated_legacy_tie_variation() {
        use super::*;
        run(&[
            "fanout",
            "equal",
            "4096",
            "budget:595000:3500000000",
            "8",
            "0",
        ]
        .map(str::to_owned))
        .unwrap();
    }

    #[test]
    fn equal_rank_siblings_form_distinct_valid_partial_templates() {
        use super::*;
        let p = Parameters {
            shape: "fanout".into(),
            fees: "equal".into(),
            count: 3,
            limit: "bytes".into(),
            repeats: 1,
            warm: 0,
        };
        let fixture = Fixture::new(&p);
        let limits = (
            fixture.entries[0].size + fixture.entries[1].size,
            fixture.total_cycles,
        );
        let left = fixture
            .validate(
                &[fixture.entries[0].clone(), fixture.entries[1].clone()],
                limits,
            )
            .unwrap();
        let right = fixture
            .validate(
                &[fixture.entries[0].clone(), fixture.entries[2].clone()],
                limits,
            )
            .unwrap();
        assert_eq!(left["selected_fees"], right["selected_fees"]);
        assert_eq!(left["selected_bytes"], right["selected_bytes"]);
        assert_ne!(left["set_digest"], right["set_digest"]);
    }

    #[test]
    fn malformed_corpus_options_fail_before_allocating() {
        use super::*;
        for args in [
            vec!["chain", "equal", "0", "all", "1", "0"],
            vec!["chain", "equal", "16385", "all", "1", "0"],
            vec!["chain", "arrival_time", "8", "all", "1", "0"],
            vec!["chain", "equal", "8", "all", "0", "0"],
            vec!["chain", "equal", "8", "all", "1", "33"],
            vec!["chain", "equal", "8", "budget:1", "1", "0"],
            vec!["chain", "equal", "8", "budget:1:2:3", "1", "0"],
            vec!["chain", "equal", "8", "budget:-1:2", "1", "0"],
        ] {
            assert!(
                Parameters::parse(&args.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    .is_err()
            );
        }
    }
}

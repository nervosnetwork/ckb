//! Small legal command sequences through the real commit functions and Store.
//!
//! The fixture supplies verified cells/cycles; canonical VM and concurrent
//! publication belong to the service tests. Expected phases and populations
//! come from the commands and a fixed three-forest corpus, not commit edits.
use super::*;
use crate::authority::chain::ClearScope;
use crate::authority::tests::common::commit_admission;
use crate::authority::{chain, ingress, jobs::Resolution, membership, waiting};
use crate::error::Reject;
use crate::service::ChainReorgArgs;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, BlockView, FeeRate, TransactionBuilder, error::OutPointError},
    packed::{CellInput, CellOutput},
};
use std::{
    any::Any,
    collections::BTreeSet,
    io::Read,
    panic::{AssertUnwindSafe, catch_unwind},
};

const TRANSACTIONS: usize = 12;
const KINDS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Receive(usize, usize),
    Resolve(usize),
    Admit(usize),
    Remove(usize),
    Expire(usize),
    Clear(bool),
    Attach,
    Detach,
}

impl Command {
    fn wire(self) -> [usize; 3] {
        match self {
            Self::Receive(id, origin) => [0, id, origin],
            Self::Resolve(id) => [1, id, 0],
            Self::Admit(id) => [2, id, 0],
            Self::Remove(id) => [3, id, 0],
            Self::Expire(id) => [4, id, 0],
            Self::Clear(pipeline) => [5, usize::from(pipeline), 0],
            Self::Attach => [6, 0, 0],
            Self::Detach => [7, 0, 0],
        }
    }

    fn from_wire(value: [usize; 3]) -> Result<Self, String> {
        let [kind, id, origin] = value;
        let command = match kind {
            0 if id < TRANSACTIONS && origin < 7 => Self::Receive(id, origin),
            1 if id < TRANSACTIONS && origin == 0 => Self::Resolve(id),
            2 if id < TRANSACTIONS && origin == 0 => Self::Admit(id),
            3 if id < TRANSACTIONS && origin == 0 => Self::Remove(id),
            4 if id < TRANSACTIONS && origin == 0 => Self::Expire(id),
            5 if id <= 1 && origin == 0 => Self::Clear(id == 1),
            6 if id == 0 && origin == 0 => Self::Attach,
            7 if id == 0 && origin == 0 => Self::Detach,
            _ => return Err(format!("invalid sequence command {value:?}")),
        };
        Ok(command)
    }
}

#[derive(Clone, Copy, Debug)]
struct ExpectedOwner {
    shape: Shape,
    origin: Origin,
}

struct Corpus {
    base: Arc<Snapshot>,
    transactions: [TransactionView; TRANSACTIONS],
    origins: [Origin; 7],
    config: ckb_app_config::TxPoolConfig,
}

impl Corpus {
    fn new() -> Self {
        let transactions = std::array::from_fn(|id| {
            let forest = id / 4;
            let root_input = OutPoint::new(Byte32::new([0xa0 + forest as u8; 32]), 0);
            let make = |input, tag| {
                TransactionBuilder::default()
                    .input(CellInput::new(input, 0))
                    .output(
                        CellOutput::new_builder()
                            .capacity(20_000_000_000u64)
                            .build(),
                    )
                    .output_data(Bytes::from(vec![tag]).pack())
                    .build()
            };
            let root = make(root_input, 0);
            let child = make(OutPoint::new(root.hash(), 0), 1);
            match id % 4 {
                0 => root,
                1 => child,
                2 => make(OutPoint::new(child.hash(), 0), 2),
                _ => make(OutPoint::new(root.hash(), 0), 3),
            }
        });
        Self {
            base: crate::test_support::genesis_snapshot(),
            transactions,
            origins: Origin::all(),
            config: ckb_app_config::TxPoolConfig {
                min_rbf_rate: FeeRate::from_u64(1_000),
                ..config()
            },
        }
    }

    fn parent(id: usize) -> Option<usize> {
        match id % 4 {
            0 => None,
            1 | 3 => Some(id / 4 * 4),
            _ => Some(id - 1),
        }
    }

    fn input(&self, id: usize) -> OutPoint {
        self.transactions[id].input_pts_iter().next().unwrap()
    }

    fn fee(id: usize) -> u64 {
        if id % 4 == 3 { 10_000 } else { 1_000 }
    }
}

struct Sequence<'a> {
    corpus: &'a Corpus,
    store: Arc<Store>,
    expected: [Option<ExpectedOwner>; TRANSACTIONS],
    attached: Option<(BlockView, Vec<usize>)>,
}

impl<'a> Sequence<'a> {
    fn new(corpus: &'a Corpus) -> Self {
        Self {
            corpus,
            store: store_with_pipeline_limit(Arc::clone(&corpus.base), &corpus.config, 64_000_000),
            expected: [None; TRANSACTIONS],
            attached: None,
        }
    }

    fn accepted(&self, id: usize) -> bool {
        self.expected[id].is_some_and(|owner| owner.shape.accepted())
    }

    fn current(&self, id: usize) -> Arc<Entry> {
        self.store
            .point(&self.corpus.transactions[id].hash())
            .1
            .expect("expected owner exists")
    }

    fn eligible(&self, command: Command) -> bool {
        match command {
            Command::Receive(id, origin) => {
                self.attached.is_none()
                    && if origin == 6 {
                        self.expected[id].is_some_and(|owner| {
                            !owner.shape.accepted()
                                && matches!(owner.origin.source, Source::Remote { .. })
                        })
                    } else {
                        self.expected[id].is_none()
                    }
            }
            Command::Resolve(id) => {
                self.expected[id].is_some_and(|owner| owner.shape == Shape::Resolve)
            }
            Command::Admit(id) => {
                self.expected[id].is_some_and(|owner| owner.shape == Shape::Verify)
                    && Corpus::parent(id).is_none_or(|parent| self.accepted(parent))
            }
            Command::Remove(id) => self.expected[id].is_some(),
            Command::Expire(id) => self.accepted(id),
            Command::Clear(pipeline) => self
                .expected
                .iter()
                .flatten()
                .any(|owner| !pipeline || !owner.shape.accepted()),
            Command::Attach => {
                self.attached.is_none()
                    && self.expected.iter().any(Option::is_some)
                    && self
                        .expected
                        .iter()
                        .flatten()
                        .all(|owner| owner.shape.accepted())
            }
            Command::Detach => self.attached.is_some(),
        }
    }

    fn descendants(&self, root: usize) -> Vec<usize> {
        let mut removed = vec![root];
        for id in root + 1..(root / 4 + 1) * 4 {
            if self.accepted(id)
                && Corpus::parent(id).is_some_and(|parent| removed.contains(&parent))
            {
                removed.push(id);
            }
        }
        removed
    }

    fn step(&mut self, command: Command) {
        assert!(
            self.eligible(command),
            "command precondition must be checked before replay"
        );
        let view = self.store.snapshot().0;
        let mut prior_premises = Premises::default();
        for tx in &self.corpus.transactions {
            self.store.get(&tx.hash(), &mut prior_premises).unwrap();
        }

        match command {
            Command::Receive(id, origin) => {
                // The proposal API supplies no peer. Promotion inherits the
                // existing remote owner; fresh proposals have no remote origin.
                let (origin, shape) = if origin == 6 {
                    let before = self.expected[id].unwrap();
                    let Source::Remote { origin: remote, .. } = before.origin.source else {
                        unreachable!("proposal promotion requires a remote owner");
                    };
                    (
                        Origin {
                            source: Source::Proposal {
                                remote: Some(remote),
                            },
                            ..before.origin
                        },
                        if before.shape == Shape::Verify {
                            Shape::Verify
                        } else {
                            Shape::Resolve
                        },
                    )
                } else {
                    (self.corpus.origins[origin], Shape::Resolve)
                };
                ingress::commit_ingress(
                    &self.store,
                    Arc::new(self.corpus.transactions[id].clone()),
                    if matches!(origin.source, Source::Proposal { .. }) {
                        Source::Proposal { remote: None }
                    } else {
                        origin.source
                    },
                )
                .unwrap();
                self.expected[id] = Some(ExpectedOwner { shape, origin });
            }
            Command::Resolve(id) => {
                let owner = self.current(id);
                let missing = Corpus::parent(id).filter(|parent| !self.accepted(*parent));
                let result = if let Some(parent) = missing {
                    let mut premises = Premises::default();
                    self.store
                        .get(&self.corpus.transactions[parent].hash(), &mut premises)
                        .unwrap();
                    let known = self.expected[parent].is_some_and(|entry| !entry.shape.history());
                    if owner.source.requires_known_producer() && !known {
                        self.expected[id] = None;
                        Resolution::Rejected(
                            Reject::Resolve(OutPointError::Unknown(self.corpus.input(id))),
                            premises,
                        )
                    } else {
                        self.expected[id].as_mut().unwrap().shape = Shape::Waiting;
                        Resolution::Waiting(BTreeSet::from([self.corpus.input(id)]), premises)
                    }
                } else {
                    self.expected[id].as_mut().unwrap().shape = Shape::Verify;
                    Resolution::Ready(Arc::clone(
                        verification_fixture(
                            &self.store,
                            &owner,
                            Corpus::fee(id),
                            19,
                            Status::Pending,
                        )
                        .resolved(),
                    ))
                };
                ingress::commit_resolution(&self.store, view, &owner, &result).unwrap();
            }
            Command::Admit(id) => {
                let conflict = (0..TRANSACTIONS).find(|other| {
                    self.accepted(*other) && self.corpus.input(*other) == self.corpus.input(id)
                });
                let (_, reject) = commit_admission(
                    &self.store,
                    &self.current(id),
                    Corpus::fee(id),
                    19,
                    Status::Pending,
                    &self.corpus.config,
                )
                .unwrap();
                if conflict.is_some_and(|other| Corpus::fee(other) > Corpus::fee(id)) {
                    assert!(
                        matches!(reject, Some(Reject::RBFRejected(_))),
                        "lower-fee replacement is rejected"
                    );
                    self.expected[id] = None;
                } else {
                    assert!(reject.is_none(), "eligible admission rejected: {reject:?}");
                    if let Some(conflict) = conflict {
                        for victim in self.descendants(conflict) {
                            self.expected[victim] = Some(ExpectedOwner {
                                shape: Shape::HistoryAll,
                                origin: Origin::trusted(Source::Recovery),
                            });
                        }
                    }
                    self.expected[id].as_mut().unwrap().shape = Shape::Pending;
                }
            }
            Command::Remove(id) | Command::Expire(id) => {
                let reason = matches!(command, Command::Expire(_)).then_some(Reject::Expiry(0));
                membership::commit_removal(
                    &self.store,
                    &self.current(id),
                    &self.corpus.config,
                    reason,
                )
                .unwrap();
                let removed = if self.accepted(id) {
                    self.descendants(id)
                } else {
                    vec![id]
                };
                for id in removed {
                    self.expected[id] = None;
                }
            }
            Command::Clear(pipeline) => {
                let scope = if pipeline {
                    ClearScope::Unaccepted
                } else {
                    ClearScope::All
                };
                chain::commit_clear(&self.store, None, scope).unwrap();
                for owner in &mut self.expected {
                    if owner.is_some_and(|owner| !pipeline || !owner.shape.accepted()) {
                        *owner = None;
                    }
                }
            }
            Command::Attach => {
                let ids: Vec<_> = (0..TRANSACTIONS).filter(|id| self.accepted(*id)).collect();
                let block = BlockBuilder::default()
                    .transaction(crate::authority::tests::common::tx(0))
                    .transactions(ids.iter().map(|id| self.corpus.transactions[*id].clone()))
                    .build();
                let snapshot = Arc::new(Snapshot::new(
                    block.header(),
                    self.corpus.base.total_difficulty().clone(),
                    self.corpus.base.epoch_ext().clone(),
                    ckb_test_chain_utils::MockStore::default()
                        .store()
                        .get_snapshot(),
                    Default::default(),
                    self.corpus.base.cloned_consensus(),
                ));
                chain::commit_reconcile(
                    &self.store,
                    &ChainReorgArgs::for_test(Default::default(), [block.clone()].into(), snapshot),
                    &self.corpus.config,
                )
                .unwrap();
                self.expected.fill(None);
                self.attached = Some((block, ids));
            }
            Command::Detach => {
                let (block, ids) = self.attached.take().unwrap();
                chain::commit_reconcile(
                    &self.store,
                    &ChainReorgArgs::for_test(
                        [block].into(),
                        Default::default(),
                        Arc::clone(&self.corpus.base),
                    ),
                    &self.corpus.config,
                )
                .unwrap();
                for id in ids {
                    self.expected[id] = Some(ExpectedOwner {
                        shape: Shape::Resolve,
                        origin: Origin::trusted(Source::Recovery),
                    });
                }
            }
        }
        // These sequences inspect owner transitions; drain unconsumed outbox
        // notices so the fixture never stalls on the outbox bound.
        self.store.outbox.drain_for_test();
        self.settle_wakes();
        self.check();
        // Every successful command changes owners or the chain view. Revalidating
        // the complete prior observation must report Stale.
        assert!(
            matches!(
                self.store.check_premises(view, &prior_premises),
                Err(Error::Stale)
            ),
            "prior complete capture is stale"
        );
        self.check();
    }

    fn settle_wakes(&mut self) {
        let mut pages = 0;
        while waiting::commit_wake(&self.store, &mut None)
            .unwrap()
            .is_some()
        {
            pages += 1;
            assert!(
                pages <= TRANSACTIONS * 2,
                "maintenance must reach a fixed point"
            );
            self.store.outbox.drain_for_test();
        }
        // Fixture parents precede their children. Waking or removing a waiter
        // leaves accepted spenders unchanged, so one forward pass settles both
        // availability and loss of a required producer.
        for id in 0..TRANSACTIONS {
            let Some(owner) = self.expected[id] else {
                continue;
            };
            if !matches!(owner.shape, Shape::Waiting | Shape::HistoryAll) {
                continue;
            }
            let parent = Corpus::parent(id).expect("only dependent fixture transactions wait");
            let spent = (0..TRANSACTIONS).any(|other| {
                self.accepted(other) && self.corpus.input(other) == self.corpus.input(id)
            });
            if self.accepted(parent) && !spent {
                self.expected[id].as_mut().unwrap().shape = Shape::Resolve;
            } else if owner.shape == Shape::Waiting
                && owner.origin.source.requires_known_producer()
                && self.expected[parent].is_none_or(|parent| parent.shape.history())
            {
                self.expected[id] = None;
            }
        }
    }

    fn check(&self) {
        let expected: Vec<_> = self
            .expected
            .iter()
            .enumerate()
            .filter_map(|(id, owner)| {
                let owner = owner.as_ref()?;
                let input = self.corpus.input(id);
                let mut relations = match owner.shape {
                    Shape::Waiting | Shape::HistoryAll => vec![ExpectedRelation::Waiter(input)],
                    Shape::Pending => vec![ExpectedRelation::Spender(input)],
                    _ => Vec::new(),
                };
                if owner.shape.accepted()
                    && let Some(parent) = Corpus::parent(id)
                {
                    relations.push(ExpectedRelation::Child(
                        self.corpus.transactions[parent].hash(),
                    ));
                }
                Some(Expected {
                    owner: self.current(id),
                    shape: owner.shape,
                    origin: owner.origin,
                    timestamp: 0,
                    relations,
                })
            })
            .collect();
        assert_state(&self.store, &expected, &[]);
    }

    fn choices(&self) -> Vec<Command> {
        let mut choices = Vec::new();
        for id in 0..TRANSACTIONS {
            for origin in 0..self.corpus.origins.len() {
                let command = Command::Receive(id, origin);
                if self.eligible(command) {
                    choices.push(command);
                }
            }
            for command in [
                Command::Resolve(id),
                Command::Admit(id),
                Command::Remove(id),
                Command::Expire(id),
            ] {
                if self.eligible(command) {
                    let weight = if matches!(command, Command::Resolve(_) | Command::Admit(_)) {
                        7
                    } else {
                        1
                    };
                    choices.extend(std::iter::repeat_n(command, weight));
                }
            }
        }
        for command in [
            Command::Clear(false),
            Command::Clear(true),
            Command::Attach,
            Command::Detach,
        ] {
            if self.eligible(command) {
                choices.push(command);
            }
        }
        assert!(!choices.is_empty());
        choices
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Replay {
    Passed,
    Invalid(usize),
    Failed {
        at: usize,
        command: Command,
        message: String,
    },
}

fn panic_message(error: Box<dyn Any + Send>) -> String {
    error
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            error
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        })
        .unwrap_or_else(|| "non-string assertion panic".into())
}

fn failure_signature(message: &str) -> &str {
    message
        .lines()
        .next()
        .filter(|line| line.contains("state oracle: "))
        .unwrap_or(message)
}

fn replay(corpus: &Corpus, commands: &[Command]) -> Replay {
    let mut sequence = Sequence::new(corpus);
    for (at, command) in commands.iter().copied().enumerate() {
        if !sequence.eligible(command) {
            return Replay::Invalid(at);
        }
        if let Err(error) = catch_unwind(AssertUnwindSafe(|| sequence.step(command))) {
            return Replay::Failed {
                at,
                command,
                message: panic_message(error),
            };
        }
    }
    Replay::Passed
}

/// Remove chunks, then single commands, preserving a caller-defined failure.
fn shrink<T: Clone>(mut commands: Vec<T>, mut reproduces: impl FnMut(&[T]) -> bool) -> Vec<T> {
    let mut width = commands.len().div_ceil(2).max(1);
    loop {
        let mut start = 0;
        while start < commands.len() {
            let end = (start + width).min(commands.len());
            let mut candidate = commands[..start].to_vec();
            candidate.extend_from_slice(&commands[end..]);
            if reproduces(&candidate) {
                commands = candidate;
                if width == 1 {
                    start = 0;
                }
            } else {
                start += width;
            }
        }
        if width == 1 {
            break;
        }
        width = width.div_ceil(2);
    }
    commands
}

fn fail(corpus: &Corpus, seed: u64, commands: Vec<Command>, failure: Replay) -> ! {
    let Replay::Failed {
        command, message, ..
    } = &failure
    else {
        panic!("{failure:?}");
    };
    let minimal = shrink(commands.clone(), |candidate| {
        matches!(replay(corpus, candidate), Replay::Failed { command: actual, message: detail, .. }
            if actual == *command && failure_signature(&detail) == failure_signature(message))
    });
    let report = serde_json::json!({
        "schema_version": 1,
        "seed": seed,
        "failure": format!("{failure:?}"),
        "failure_signature": failure_signature(message),
        "original_commands": commands.iter().copied().map(Command::wire).collect::<Vec<_>>(),
        "commands": minimal.into_iter().map(Command::wire).collect::<Vec<_>>(),
    });
    eprintln!("TX_POOL_SEQUENCE_FAILURE {report}");
    panic!("state sequence failed; seed={seed}; replay the commands in TX_POOL_SEQUENCE_FAILURE");
}

fn generated(corpus: &Corpus, seed: u64, steps: usize) -> [usize; KINDS] {
    let mut sequence = Sequence::new(corpus);
    let mut random = seed;
    let mut commands = Vec::with_capacity(steps);
    let mut counts = [0; KINDS];
    let root = (seed % 3) as usize * 4;
    let prefix = [
        Command::Receive(root, (seed % 6) as usize),
        Command::Resolve(root),
        Command::Admit(root),
        Command::Attach,
        Command::Detach,
    ];
    for step in 0..steps {
        random = random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let command = prefix.get(step).copied().unwrap_or_else(|| {
            let choices = sequence.choices();
            choices[((random >> 32) % choices.len() as u64) as usize]
        });
        counts[command.wire()[0]] += 1;
        commands.push(command);
        if let Err(error) = catch_unwind(AssertUnwindSafe(|| sequence.step(command))) {
            let failure = Replay::Failed {
                at: commands.len() - 1,
                command,
                message: panic_message(error),
            };
            fail(corpus, seed, commands, failure);
        }
    }
    counts
}

#[test]
fn generated_sequences_preserve_population_and_reject_stale_cuts() {
    let corpus = Corpus::new();
    let mut total = [0; KINDS];
    for seed in 0..16 {
        let counts = generated(&corpus, seed, 512);
        for (total, count) in total.iter_mut().zip(counts) {
            *total += count;
        }
    }
    assert!(
        total.iter().all(|count| *count > 0),
        "all command kinds execute: {total:?}"
    );
}

#[test]
fn sequence_replay_rejects_invalid_commands_and_shrinks_a_legal_failure() {
    assert!(Command::from_wire([0, TRANSACTIONS, 0]).is_err());
    assert!(Command::from_wire([6, 1, 0]).is_err());
    let corpus = Corpus::new();
    assert_eq!(replay(&corpus, &[Command::Admit(0)]), Replay::Invalid(0));
    let commands = vec![
        Command::Receive(4, 0),
        Command::Receive(0, 0),
        Command::Resolve(0),
        Command::Admit(0),
        Command::Remove(4),
    ];
    let reproduces = |commands: &[Command]| {
        replay(&corpus, commands) == Replay::Passed && commands.contains(&Command::Admit(0))
    };
    let minimal = shrink(commands, reproduces);
    assert_eq!(
        minimal,
        [
            Command::Receive(0, 0),
            Command::Resolve(0),
            Command::Admit(0)
        ]
    );
    for index in 0..minimal.len() {
        let mut shorter = minimal.clone();
        shorter.remove(index);
        assert!(!reproduces(&shorter));
    }
}

#[test]
#[ignore = "extended deterministic sequences or explicit JSON replay"]
fn extended_state_sequences() {
    let corpus = Corpus::new();
    if let Ok(path) = std::env::var("TX_POOL_SEQUENCE_REPLAY") {
        const MAX_REPLAY_BYTES: u64 = 8 * 1024 * 1024;
        let mut input = Vec::new();
        std::fs::File::open(path)
            .unwrap()
            .take(MAX_REPLAY_BYTES + 1)
            .read_to_end(&mut input)
            .unwrap();
        assert!(
            input.len() as u64 <= MAX_REPLAY_BYTES,
            "replay file is too large"
        );
        let value: serde_json::Value = serde_json::from_slice(&input).unwrap();
        assert_eq!(value["schema_version"], 1);
        let encoded: Vec<[usize; 3]> = serde_json::from_value(value["commands"].clone()).unwrap();
        assert!(encoded.len() <= 65_536, "replay input is bounded");
        let commands: Vec<_> = encoded
            .into_iter()
            .map(|value| Command::from_wire(value).unwrap())
            .collect();
        assert_eq!(replay(&corpus, &commands), Replay::Passed);
    } else {
        let first: u64 =
            std::env::var("TX_POOL_SEQUENCE_SEED").map_or(0, |value| value.parse().unwrap());
        let mut total = [0; KINDS];
        for offset in 0..64 {
            let counts = generated(&corpus, first.wrapping_add(offset), 4_096);
            for (total, count) in total.iter_mut().zip(counts) {
                *total += count;
            }
        }
        println!(
            "TX_POOL_STATE_SEQUENCES {}",
            serde_json::json!({"schema_version": 1, "first_seed": first, "seeds": 64, "steps_per_seed": 4096, "command_counts": total})
        );
    }
}

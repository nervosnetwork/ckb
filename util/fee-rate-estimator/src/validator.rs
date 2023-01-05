use crate::{helper::PrettyDisplay, types};
use ckb_types::packed;
use faketime::unix_time;
use std::{collections::HashMap, time::Duration};

struct Prediction<T>
where
    T: Clone + Copy + Eq + Ord + PartialEq + PartialOrd,
{
    added_dt: Duration,
    expected: T,
}

pub(crate) struct Validator<T>
where
    T: Clone + Copy + Eq + Ord + PartialEq + PartialOrd,
{
    name: &'static str,
    predictions: HashMap<packed::Byte32, Prediction<T>>,
    failure: Vec<Duration>,
    success: Vec<Duration>,
}

impl Validator<Duration> {
    pub(crate) fn target_minutes() -> &'static [u32] {
        &[5, 10, 30, 60, 60 * 2, 60 * 3, 60 * 6, 60 * 12, 60 * 24]
    }
}

impl Validator<u64> {
    pub(crate) fn target_blocks() -> &'static [u32] {
        &[10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000]
    }
}

impl<T> Validator<T>
where
    T: Clone + Copy + Eq + Ord + PartialEq + PartialOrd,
{
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            predictions: HashMap::new(),
            failure: Vec::new(),
            success: Vec::new(),
        }
    }

    fn score_interval_minutes() -> &'static [u64] {
        &[30, 60, 60 * 2, 60 * 4, 60 * 12, 60 * 24]
    }

    pub(crate) fn predict(&mut self, tx_hash: packed::Byte32, added_dt: Duration, expected: T) {
        let prediction = Prediction { added_dt, expected };
        self.predictions.insert(tx_hash, prediction);
    }

    pub(crate) fn expire(&mut self, current: T) {
        let mut failure = Vec::new();
        for hash in self.predictions.keys() {
            let expected = self.predictions.get(hash).unwrap().expected;
            if current > expected {
                failure.push(hash.to_owned());
            }
        }
        for hash in failure {
            let tx = self.predictions.remove(&hash).unwrap();
            self.failure.push(tx.added_dt);
        }
    }

    pub(crate) fn confirm(&mut self, block: &types::Block) {
        let mut success = Vec::new();
        for hash in block.tx_hashes() {
            if self.predictions.contains_key(hash) {
                success.push(hash.to_owned());
            }
        }
        for hash in success {
            let tx = self.predictions.remove(&hash).unwrap();
            self.success.push(tx.added_dt);
        }
    }

    pub(crate) fn reject(&mut self, tx: &types::RejectedTransaction) {
        if self.predictions.contains_key(&tx.hash()) {
            if tx.is_invalid() {
                ckb_logger::trace!("reject-tx: remove since reason is{}", tx.reason());
                self.predictions.remove(&tx.hash()).unwrap();
            } else {
                ckb_logger::trace!("reject-tx: keep   since reason is {}", tx.reason());
            }
        } else {
            ckb_logger::trace!("reject-tx: doesn't have this transaction {:#x}", tx.hash());
        }
    }

    pub(crate) fn score(&self, after_dt_opt: Option<Duration>) -> (usize, usize) {
        if let Some(after_dt) = after_dt_opt {
            let f = |vec: &[Duration]| vec.iter().filter(|added_dt| *added_dt >= &after_dt).count();
            (f(&self.success), f(&self.failure))
        } else {
            let f = Vec::len;
            (f(&self.success), f(&self.failure))
        }
    }

    pub(crate) fn trace_score(&self) {
        let current_dt = unix_time();
        for minutes in Self::score_interval_minutes() {
            let after_dt = current_dt - Duration::from_secs(*minutes * 60);
            let (success_cnt, failure_cnt) = self.score(Some(after_dt));
            let total_cnt = success_cnt + failure_cnt;
            if total_cnt == 0 {
                return;
            }
            let accuracy = f64::from(success_cnt as u32) / f64::from(total_cnt as u32);
            ckb_logger::trace!(
                "validate [{}] in {:4} minutes: accuracy: {:.2} for {} records (since {})",
                self.name,
                minutes,
                accuracy,
                total_cnt,
                after_dt.pretty()
            );
        }
        let (success_cnt, failure_cnt) = self.score(None);
        let total_cnt = success_cnt + failure_cnt;
        if total_cnt == 0 {
            return;
        }
        let accuracy = f64::from(success_cnt as u32) / f64::from(total_cnt as u32);
        ckb_logger::info!(
            "validate [{}] for all records: accuracy: {:.2} for {} records",
            self.name,
            accuracy,
            total_cnt
        );
    }
}

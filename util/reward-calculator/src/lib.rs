use ckb_core::header::{BlockNumber, Header};
use ckb_core::script::Script;
use ckb_core::transaction::ProposalShortId;
use ckb_core::Capacity;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use failure::{Error as FailureError, Fail};
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use std::cmp;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Error {
    #[fail(display = "Can't resolve finalize target: {}", _0)]
    Target(BlockNumber),
    #[fail(display = "Can't parse Script from target witness: {:x}", _0)]
    Script(H256),
}

pub struct RewardCalculator<'a, P> {
    pub provider: &'a P,
}

impl<'a, P: ChainProvider> RewardCalculator<'a, P> {
    pub fn new(provider: &'a P) -> Self {
        RewardCalculator { provider }
    }

    pub fn block_reward(&self, parent: &Header) -> Result<(Script, Capacity), FailureError> {
        let consensus = self.provider.consensus();
        let store = self.provider.store();

        let block_number = parent.number() + 1;
        let target_number = consensus
            .finalize_target(block_number)
            .ok_or_else(|| Error::Target(block_number))?;

        let target = self
            .provider
            .get_ancestor(parent.hash(), target_number)
            .ok_or_else(|| Error::Target(block_number))?;

        let target_lock = Script::from_witness(
            &store
                .get_cellbase(target.hash())
                .expect("target cellbase exist")
                .witnesses()[0],
        )
        .ok_or_else(|| Error::Script(target.hash().to_owned()))?;

        let txs_fees = self.txs_fees(&target)?;
        let proposal_reward = self.proposal_reward(parent, &target)?;
        let base_block_reward = self.base_block_reward(&target)?;

        let reward = txs_fees
            .safe_add(proposal_reward)?
            .safe_add(base_block_reward)?;
        Ok((target_lock, reward))
    }

    pub fn txs_fees(&self, target: &Header) -> Result<Capacity, FailureError> {
        let target_ext = self
            .provider
            .store()
            .get_block_ext(target.hash())
            .expect("block body stored");

        target_ext
            .txs_fees
            .iter()
            .try_fold(Capacity::zero(), |acc, tx_fee| {
                tx_fee.safe_mul_ratio(4, 10).and_then(|proposer| {
                    tx_fee
                        .safe_sub(proposer)
                        .and_then(|miner| acc.safe_add(miner))
                })
            })
            .map_err(Into::into)
    }

    pub fn proposal_reward(
        &self,
        parent: &Header,
        target: &Header,
    ) -> Result<Capacity, FailureError> {
        let mut target_proposals = self.get_proposal_ids_by_hash(target.hash());

        let proposal_window = self.provider.consensus().tx_proposal_window();
        let block_number = parent.number() + 1;
        let store = self.provider.store();

        let mut reward = Capacity::zero();
        let commit_start = cmp::max(block_number.saturating_sub(proposal_window.length()), 2);
        let proposal_start = cmp::max(commit_start.saturating_sub(proposal_window.start()), 1);

        let mut proposal_table = BTreeMap::new();
        for bn in proposal_start..target.number() {
            let proposals = store
                .get_block_hash(bn)
                .map(|hash| self.get_proposal_ids_by_hash(&hash))
                .expect("finalize target exist");
            proposal_table.insert(bn, proposals);
        }

        let mut index = parent.to_owned();
        for (id, tx_fee) in store
            .get_block_txs_hashes(index.hash())
            .expect("block body stored")
            .iter()
            .skip(1)
            .map(ProposalShortId::from_tx_hash)
            .zip(
                store
                    .get_block_ext(index.hash())
                    .expect("block body stored")
                    .txs_fees
                    .iter(),
            )
        {
            if target_proposals.remove(&id) {
                reward = reward.safe_add(tx_fee.safe_mul_ratio(4, 10)?)?;
            }
        }

        index = store
            .get_block_header(index.parent_hash())
            .expect("header stored");

        while index.number() >= commit_start {
            let proposal_start =
                cmp::max(index.number().saturating_sub(proposal_window.start()), 1);
            let previous_ids: FnvHashSet<ProposalShortId> = proposal_table
                .range(proposal_start..)
                .flat_map(|(_, ids)| ids.iter().cloned())
                .collect();
            for (id, tx_fee) in store
                .get_block_txs_hashes(index.hash())
                .expect("block body stored")
                .iter()
                .skip(1)
                .map(ProposalShortId::from_tx_hash)
                .zip(
                    store
                        .get_block_ext(index.hash())
                        .expect("block body stored")
                        .txs_fees
                        .iter(),
                )
            {
                if target_proposals.remove(&id) && !previous_ids.contains(&id) {
                    reward = reward.safe_add(tx_fee.safe_mul_ratio(4, 10)?)?;
                }
            }

            index = store
                .get_block_header(index.parent_hash())
                .expect("header stored");
        }
        Ok(reward)
    }

    fn base_block_reward(&self, target: &Header) -> Result<Capacity, FailureError> {
        let target_parent_hash = target.parent_hash();
        let target_parent_epoch = self
            .provider
            .get_block_epoch(target_parent_hash)
            .expect("target parent exist");
        let target_parent = self
            .provider
            .store()
            .get_block_header(target_parent_hash)
            .expect("target parent exist");
        let epoch = self
            .provider
            .next_epoch_ext(&target_parent_epoch, &target_parent)
            .unwrap_or(target_parent_epoch);

        epoch.block_reward(target.number()).map_err(Into::into)
    }

    fn get_proposal_ids_by_hash(&self, hash: &H256) -> FnvHashSet<ProposalShortId> {
        let mut ids_set = FnvHashSet::default();
        if let Some(ids) = self.provider.store().get_block_proposal_txs_ids(&hash) {
            ids_set.extend(ids)
        }
        if let Some(us) = self.provider.store().get_block_uncles(&hash) {
            for u in us {
                ids_set.extend(u.proposals);
            }
        }
        ids_set
    }
}

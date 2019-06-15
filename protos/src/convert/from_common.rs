use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

use ckb_core::{
    extras::{BlockExt, DaoStats, EpochExt, TransactionInfo},
    header::{Header, HeaderBuilder},
    script::Script,
    transaction::{
        CellInput, CellOutPoint, CellOutput, OutPoint, ProposalShortId, Transaction,
        TransactionBuilder, Witness,
    },
    transaction_meta::{TransactionMeta, TransactionMetaBuilder},
    uncle::UncleBlock,
    Bytes, Capacity,
};

use crate as protos;
use crate::convert::FbVecIntoIterator;

impl From<&protos::Bytes32> for H256 {
    fn from(h256: &protos::Bytes32) -> H256 {
        cast!(H256::from_slice(&[
            h256.u0(),
            h256.u1(),
            h256.u2(),
            h256.u3(),
            h256.u4(),
            h256.u5(),
            h256.u6(),
            h256.u7(),
            h256.u8_(),
            h256.u9(),
            h256.u10(),
            h256.u11(),
            h256.u12(),
            h256.u13(),
            h256.u14(),
            h256.u15(),
            h256.u16_(),
            h256.u17(),
            h256.u18(),
            h256.u19(),
            h256.u20(),
            h256.u21(),
            h256.u22(),
            h256.u23(),
            h256.u24(),
            h256.u25(),
            h256.u26(),
            h256.u27(),
            h256.u28(),
            h256.u29(),
            h256.u30(),
            h256.u31(),
        ]))
    }
}

impl From<&protos::Bytes32> for U256 {
    fn from(h256: &protos::Bytes32) -> U256 {
        let bytes = [
            h256.u0(),
            h256.u1(),
            h256.u2(),
            h256.u3(),
            h256.u4(),
            h256.u5(),
            h256.u6(),
            h256.u7(),
            h256.u8_(),
            h256.u9(),
            h256.u10(),
            h256.u11(),
            h256.u12(),
            h256.u13(),
            h256.u14(),
            h256.u15(),
            h256.u16_(),
            h256.u17(),
            h256.u18(),
            h256.u19(),
            h256.u20(),
            h256.u21(),
            h256.u22(),
            h256.u23(),
            h256.u24(),
            h256.u25(),
            h256.u26(),
            h256.u27(),
            h256.u28(),
            h256.u29(),
            h256.u30(),
            h256.u31(),
        ];
        cast!(U256::from_little_endian(&bytes))
    }
}

impl From<&protos::ProposalShortId> for ProposalShortId {
    fn from(short_id: &protos::ProposalShortId) -> Self {
        cast!(ProposalShortId::from_slice(&[
            short_id.u0(),
            short_id.u1(),
            short_id.u2(),
            short_id.u3(),
            short_id.u4(),
            short_id.u5(),
            short_id.u6(),
            short_id.u7(),
            short_id.u8_(),
            short_id.u9(),
        ]))
    }
}

impl From<&protos::TransactionInfo> for TransactionInfo {
    fn from(proto: &protos::TransactionInfo) -> Self {
        let block_hash = proto.block_hash().into();
        let block_number = proto.block_number();
        let block_epoch = proto.block_epoch();
        let index = proto.index() as usize;
        TransactionInfo {
            block_hash,
            block_number,
            block_epoch,
            index,
        }
    }
}

impl<'a> protos::Header<'a> {
    pub fn build_unchecked(&self, hash: H256) -> Header {
        let parent_hash = cast!(self.parent_hash());
        let transactions_root = cast!(self.transactions_root());
        let witnesses_root = cast!(self.witnesses_root());
        let proposals_hash = cast!(self.proposals_hash());
        let uncles_hash = cast!(self.uncles_hash());

        let difficulty = cast!(self.difficulty()).into();
        let proof = cast!(self.proof().and_then(|p| p.seq()).map(Bytes::from));

        let builder = HeaderBuilder::default()
            .version(self.version())
            .parent_hash(parent_hash.into())
            .timestamp(self.timestamp())
            .number(self.number())
            .epoch(self.epoch())
            .transactions_root(transactions_root.into())
            .witnesses_root(witnesses_root.into())
            .proposals_hash(proposals_hash.into())
            .difficulty(difficulty)
            .uncles_hash(uncles_hash.into())
            .nonce(self.nonce())
            .proof(proof)
            .uncles_count(self.uncles_count());

        unsafe { builder.build_unchecked(hash) }
    }
}

impl<'a> protos::UncleBlock<'a> {
    pub fn build_unchecked(&self, hash: H256) -> UncleBlock {
        let proposals = cast!(self.proposals()).iter().map(Into::into).collect();
        let raw_header = cast!(self.header());
        let header = raw_header.build_unchecked(hash);
        UncleBlock { header, proposals }
    }
}

impl<'a> protos::Transaction<'a> {
    pub fn build_unchecked(&self, hash: H256, witness_hash: H256) -> Transaction {
        let deps = cast!(self.deps()).iter();
        let inputs = cast!(self.inputs()).iter();
        let outputs = cast!(self.outputs()).iter();
        let witnesses = cast!(self.witnesses()).iter();
        let builder = TransactionBuilder::default()
            .version(self.version())
            .deps(deps)
            .inputs(inputs)
            .outputs(outputs)
            .witnesses(witnesses);
        unsafe { builder.build_unchecked(hash, witness_hash) }
    }
}

impl<'a> From<protos::OutPoint<'a>> for OutPoint {
    fn from(out_point: protos::OutPoint<'a>) -> Self {
        let cell = out_point.tx_hash().map(|tx_hash| CellOutPoint {
            tx_hash: tx_hash.into(),
            index: out_point.index(),
        });
        let block_hash = out_point.block_hash().map(Into::into);
        OutPoint { block_hash, cell }
    }
}

impl<'a> From<protos::Witness<'a>> for Witness {
    fn from(witness: protos::Witness<'a>) -> Self {
        cast!(witness.data())
            .iter()
            .map(|item| cast!(item.seq().map(Bytes::from)))
            .collect()
    }
}

impl<'a> From<protos::Script<'a>> for Script {
    fn from(script: protos::Script<'a>) -> Self {
        let args = cast!(script.args())
            .iter()
            .map(|item| cast!(item.seq().map(Bytes::from)))
            .collect();
        let code_hash = cast!(script.code_hash()).into();
        Script { args, code_hash }
    }
}

impl<'a> From<protos::CellInput<'a>> for CellInput {
    fn from(cell_input: protos::CellInput<'a>) -> Self {
        let cell = cell_input.tx_hash().map(|tx_hash| CellOutPoint {
            tx_hash: tx_hash.into(),
            index: cell_input.index(),
        });
        let block_hash = cell_input.block_hash().map(Into::into);
        let previous_output = OutPoint { block_hash, cell };
        CellInput {
            previous_output,
            since: cell_input.since(),
        }
    }
}

impl<'a> From<protos::CellOutput<'a>> for CellOutput {
    fn from(cell_output: protos::CellOutput<'a>) -> Self {
        let lock = cast!(cell_output.lock());
        let type_ = cell_output.type_().map(Into::into);
        CellOutput {
            capacity: Capacity::shannons(cell_output.capacity()),
            data: Bytes::from(cast!(cell_output.data().and_then(|s| s.seq()))),
            lock: lock.into(),
            type_,
        }
    }
}

impl From<&protos::DaoStats> for DaoStats {
    fn from(proto: &protos::DaoStats) -> Self {
        let accumulated_rate = proto.accumulated_rate();
        let accumulated_capacity = proto.accumulated_capacity();
        DaoStats {
            accumulated_rate,
            accumulated_capacity,
        }
    }
}

impl<'a> From<protos::BlockExt<'a>> for BlockExt {
    fn from(proto: protos::BlockExt<'a>) -> Self {
        let received_at = proto.received_at();
        let total_difficulty = cast!(proto.total_difficulty()).into();
        let total_uncles_count = proto.total_uncles_count();
        let verified = if proto.has_verified() {
            Some(proto.verified())
        } else {
            None
        };
        let dao_stats = cast!(proto.dao_stats()).into();
        let txs_fees = cast!(proto.txs_fees())
            .iter()
            .map(Capacity::shannons)
            .collect::<Vec<_>>();
        BlockExt {
            received_at,
            total_difficulty,
            total_uncles_count,
            verified,
            dao_stats,
            txs_fees,
        }
    }
}

impl From<&protos::EpochExt> for EpochExt {
    fn from(proto: &protos::EpochExt) -> Self {
        let number = proto.number();
        let block_reward = Capacity::shannons(proto.block_reward());
        let remainder_reward = Capacity::shannons(proto.remainder_reward());
        let last_block_hash_in_previous_epoch = proto.last_block_hash_in_previous_epoch().into();
        let start_number = proto.start_number();
        let length = proto.length();
        let difficulty = proto.difficulty().into();
        EpochExt::new(
            number,
            block_reward,
            remainder_reward,
            last_block_hash_in_previous_epoch,
            start_number,
            length,
            difficulty,
        )
    }
}

impl<'a> From<protos::TransactionMeta<'a>> for TransactionMeta {
    fn from(proto: protos::TransactionMeta<'a>) -> Self {
        let bits = cast!(proto.bits().and_then(|p| p.seq()).map(ToOwned::to_owned));
        TransactionMetaBuilder::default()
            .block_number(proto.block_number())
            .epoch_number(proto.epoch_number())
            .cellbase(proto.cellbase())
            .bits(bits)
            .len(proto.len() as usize)
            .build()
    }
}

impl From<&protos::CellMeta> for (Capacity, H256) {
    fn from(proto: &protos::CellMeta) -> Self {
        let capacity = Capacity::shannons(proto.capacity());
        let data_hash = proto.data_hash().into();
        (capacity, data_hash)
    }
}

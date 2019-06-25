use std::convert::{TryFrom, TryInto};

use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

use ckb_core::{
    extras::{BlockExt, EpochExt, TransactionInfo},
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

use crate::convert::{FbVecIntoIterator, OptionShouldBeSome};
use crate::{
    self as protos,
    error::{Error, Result},
};

impl TryFrom<&protos::Bytes32> for H256 {
    type Error = Error;
    fn try_from(h256: &protos::Bytes32) -> Result<Self> {
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
        H256::from_slice(&bytes).ok().unwrap_some()
    }
}

impl TryFrom<&protos::Bytes32> for U256 {
    type Error = Error;
    fn try_from(h256: &protos::Bytes32) -> Result<Self> {
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
        U256::from_little_endian(&bytes).ok().unwrap_some()
    }
}

impl TryFrom<&protos::ProposalShortId> for ProposalShortId {
    type Error = Error;
    fn try_from(short_id: &protos::ProposalShortId) -> Result<Self> {
        let bytes = [
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
        ];
        ProposalShortId::from_slice(&bytes).unwrap_some()
    }
}

impl TryFrom<&protos::TransactionInfo> for TransactionInfo {
    type Error = Error;
    fn try_from(proto: &protos::TransactionInfo) -> Result<Self> {
        let block_hash = proto.block_hash().try_into()?;
        let block_number = proto.block_number();
        let block_epoch = proto.block_epoch();
        let index = proto.index() as usize;
        let ret = TransactionInfo {
            block_hash,
            block_number,
            block_epoch,
            index,
        };
        Ok(ret)
    }
}

impl<'a> protos::Header<'a> {
    pub fn build_unchecked(&self, hash: H256) -> Result<Header> {
        let parent_hash = self.parent_hash().unwrap_some()?;
        let transactions_root = self.transactions_root().unwrap_some()?;
        let witnesses_root = self.witnesses_root().unwrap_some()?;
        let proposals_hash = self.proposals_hash().unwrap_some()?;
        let uncles_hash = self.uncles_hash().unwrap_some()?;

        let difficulty = self.difficulty().unwrap_some()?.try_into()?;
        let proof = self
            .proof()
            .and_then(|p| p.seq())
            .map(Bytes::from)
            .unwrap_some()?;

        let dao = self
            .dao()
            .and_then(|d| d.seq())
            .map(Bytes::from)
            .unwrap_some()?;

        let builder = HeaderBuilder::default()
            .version(self.version())
            .parent_hash(parent_hash.try_into()?)
            .timestamp(self.timestamp())
            .number(self.number())
            .epoch(self.epoch())
            .transactions_root(transactions_root.try_into()?)
            .witnesses_root(witnesses_root.try_into()?)
            .proposals_hash(proposals_hash.try_into()?)
            .difficulty(difficulty)
            .uncles_hash(uncles_hash.try_into()?)
            .nonce(self.nonce())
            .proof(proof)
            .dao(dao)
            .uncles_count(self.uncles_count());

        let header = unsafe { builder.build_unchecked(hash) };
        Ok(header)
    }
}

impl<'a> protos::UncleBlock<'a> {
    pub fn build_unchecked(&self, hash: H256) -> Result<UncleBlock> {
        let proposals = self
            .proposals()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>>>()?;
        let raw_header = self.header().unwrap_some()?;
        let header = raw_header.build_unchecked(hash)?;
        let uncle = UncleBlock { header, proposals };
        Ok(uncle)
    }
}

impl<'a> protos::Transaction<'a> {
    pub fn build_unchecked(&self, hash: H256, witness_hash: H256) -> Result<Transaction> {
        let deps = self
            .deps()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<OutPoint>>>()?;
        let inputs = self
            .inputs()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<CellInput>>>()?;
        let outputs = self
            .outputs()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<CellOutput>>>()?;
        let witnesses = self
            .witnesses()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<Witness>>>()?;
        let builder = TransactionBuilder::default()
            .version(self.version())
            .deps(deps)
            .inputs(inputs)
            .outputs(outputs)
            .witnesses(witnesses);
        let transaction = unsafe { builder.build_unchecked(hash, witness_hash) };
        Ok(transaction)
    }
}

impl<'a> TryFrom<protos::OutPoint<'a>> for OutPoint {
    type Error = Error;
    fn try_from(out_point: protos::OutPoint<'a>) -> Result<Self> {
        let cell = if let Some(tx_hash) = out_point.tx_hash() {
            let cell = CellOutPoint {
                tx_hash: tx_hash.try_into()?,
                index: out_point.index(),
            };
            Some(cell)
        } else {
            None
        };
        let block_hash = out_point.block_hash().map(TryInto::try_into).transpose()?;
        let ret = OutPoint { block_hash, cell };
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::Witness<'a>> for Witness {
    type Error = Error;
    fn try_from(witness: protos::Witness<'a>) -> Result<Self> {
        witness
            .data()
            .unwrap_some()?
            .iter()
            .map(|item| item.seq().map(Bytes::from))
            .collect::<Option<Witness>>()
            .unwrap_some()
    }
}

impl<'a> TryFrom<protos::Script<'a>> for Script {
    type Error = Error;
    fn try_from(script: protos::Script<'a>) -> Result<Self> {
        let args = script
            .args()
            .unwrap_some()?
            .iter()
            .map(|item| item.seq().map(Bytes::from))
            .collect::<Option<_>>()
            .unwrap_some()?;
        let code_hash = script.code_hash().unwrap_some()?.try_into()?;
        let ret = Script { args, code_hash };
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::CellInput<'a>> for CellInput {
    type Error = Error;
    fn try_from(cell_input: protos::CellInput<'a>) -> Result<Self> {
        let cell = if let Some(tx_hash) = cell_input.tx_hash() {
            let cell = CellOutPoint {
                tx_hash: tx_hash.try_into()?,
                index: cell_input.index(),
            };
            Some(cell)
        } else {
            None
        };
        let block_hash = cell_input.block_hash().map(TryInto::try_into).transpose()?;
        let previous_output = OutPoint { block_hash, cell };
        let ret = CellInput {
            previous_output,
            since: cell_input.since(),
        };
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::CellOutput<'a>> for CellOutput {
    type Error = Error;
    fn try_from(cell_output: protos::CellOutput<'a>) -> Result<Self> {
        let lock = cell_output.lock().unwrap_some()?;
        let type_ = cell_output.type_().map(TryInto::try_into).transpose()?;
        let data = cell_output.data().and_then(|s| s.seq()).unwrap_some()?;
        let ret = CellOutput {
            capacity: Capacity::shannons(cell_output.capacity()),
            data: Bytes::from(data),
            lock: lock.try_into()?,
            type_,
        };
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::BlockExt<'a>> for BlockExt {
    type Error = Error;
    fn try_from(proto: protos::BlockExt<'a>) -> Result<Self> {
        let received_at = proto.received_at();
        let total_difficulty = proto.total_difficulty().unwrap_some()?.try_into()?;
        let total_uncles_count = proto.total_uncles_count();
        let verified = if proto.has_verified() {
            Some(proto.verified())
        } else {
            None
        };
        let txs_fees = proto
            .txs_fees()
            .unwrap_some()?
            .iter()
            .map(Capacity::shannons)
            .collect::<Vec<_>>();
        let ret = BlockExt {
            received_at,
            total_difficulty,
            total_uncles_count,
            verified,
            txs_fees,
        };
        Ok(ret)
    }
}

impl TryFrom<&protos::EpochExt> for EpochExt {
    type Error = Error;
    fn try_from(proto: &protos::EpochExt) -> Result<Self> {
        let number = proto.number();
        let block_reward = Capacity::shannons(proto.block_reward());
        let remainder_reward = Capacity::shannons(proto.remainder_reward());
        let last_block_hash_in_previous_epoch =
            proto.last_block_hash_in_previous_epoch().try_into()?;
        let start_number = proto.start_number();
        let length = proto.length();
        let difficulty = proto.difficulty().try_into()?;
        let ret = EpochExt::new(
            number,
            block_reward,
            remainder_reward,
            last_block_hash_in_previous_epoch,
            start_number,
            length,
            difficulty,
        );
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::TransactionMeta<'a>> for TransactionMeta {
    type Error = Error;
    fn try_from(proto: protos::TransactionMeta<'a>) -> Result<Self> {
        let bits = proto
            .bits()
            .and_then(|p| p.seq())
            .map(ToOwned::to_owned)
            .unwrap_some()?;
        let ret = TransactionMetaBuilder::default()
            .block_number(proto.block_number())
            .epoch_number(proto.epoch_number())
            .cellbase(proto.cellbase())
            .bits(bits)
            .len(proto.len() as usize)
            .build();
        Ok(ret)
    }
}

impl TryFrom<&protos::CellMeta> for (Capacity, H256) {
    type Error = Error;
    fn try_from(proto: &protos::CellMeta) -> Result<Self> {
        let capacity = Capacity::shannons(proto.capacity());
        let data_hash = proto.data_hash().try_into()?;
        let ret = (capacity, data_hash);
        Ok(ret)
    }
}

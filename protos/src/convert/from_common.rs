use std::convert::{TryFrom, TryInto};

use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

use ckb_core::{
    extras::{BlockExt, EpochExt, TransactionInfo},
    header::{Header, HeaderBuilder},
    script::Script,
    transaction::{
        CellDep, CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
        Witness,
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
        let cell_deps = self
            .cell_deps()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<CellDep>>>()?;
        let header_deps = self
            .header_deps()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<H256>>>()?;

        let inputs = self
            .inputs()
            .unwrap_some()?
            .iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<CellInput>>>()?;
        let outputs_data = self
            .outputs_data()
            .unwrap_some()?
            .iter()
            .map(|item| item.seq().map(Bytes::from))
            .collect::<Option<Vec<Bytes>>>()
            .unwrap_some()?;
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
            .cell_deps(cell_deps)
            .header_deps(header_deps)
            .inputs(inputs)
            .outputs(outputs)
            .outputs_data(outputs_data)
            .witnesses(witnesses);
        let transaction = unsafe { builder.build_unchecked(hash, witness_hash) };
        Ok(transaction)
    }
}

impl<'a> TryFrom<protos::CellDep<'a>> for CellDep {
    type Error = Error;
    fn try_from(dep: protos::CellDep<'a>) -> Result<Self> {
        if dep.is_dep_group() > 1 {
            return Err(Error::Deserialize);
        }
        let tx_hash = dep.tx_hash().unwrap_some()?;
        let out_point = OutPoint::new(tx_hash.try_into()?, dep.index());
        let is_dep_group = dep.is_dep_group() == 1;
        Ok(CellDep::new(out_point, is_dep_group))
    }
}

impl<'a> TryFrom<protos::OutPoint<'a>> for OutPoint {
    type Error = Error;
    fn try_from(out_point: protos::OutPoint<'a>) -> Result<Self> {
        let tx_hash = out_point.tx_hash().unwrap_some()?;
        Ok(OutPoint::new(tx_hash.try_into()?, out_point.index()))
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
        let hash_type = script
            .hash_type()
            .try_into()
            .map_err(|_| Error::Deserialize)?;
        let ret = Script {
            args,
            code_hash,
            hash_type,
        };
        Ok(ret)
    }
}

impl<'a> TryFrom<protos::CellInput<'a>> for CellInput {
    type Error = Error;
    fn try_from(cell_input: protos::CellInput<'a>) -> Result<Self> {
        let tx_hash = cell_input.tx_hash().unwrap_some()?;
        let index = cell_input.index();
        let out_point = OutPoint::new(tx_hash.try_into()?, index);
        Ok(CellInput {
            previous_output: out_point,
            since: cell_input.since(),
        })
    }
}

impl<'a> TryFrom<protos::CellOutput<'a>> for CellOutput {
    type Error = Error;
    fn try_from(cell_output: protos::CellOutput<'a>) -> Result<Self> {
        let lock = cell_output.lock().unwrap_some()?;
        let type_ = cell_output.type_().map(TryInto::try_into).transpose()?;
        let data_hash = cell_output.data_hash().unwrap_some()?;
        let ret = CellOutput {
            capacity: Capacity::shannons(cell_output.capacity()),
            data_hash: data_hash.try_into()?,
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
        let previous_epoch_hash_rate = proto.previous_epoch_hash_rate().try_into()?;
        let ret = EpochExt::new(
            number,
            block_reward,
            remainder_reward,
            previous_epoch_hash_rate,
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
        let block_hash = proto.block_hash().unwrap_some()?;
        let bits = proto
            .bits()
            .and_then(|p| p.seq())
            .map(ToOwned::to_owned)
            .unwrap_some()?;
        let ret = TransactionMetaBuilder::default()
            .block_number(proto.block_number())
            .epoch_number(proto.epoch_number())
            .block_hash(block_hash.try_into()?)
            .cellbase(proto.cellbase())
            .bits(bits)
            .len(proto.len() as usize)
            .build();
        Ok(ret)
    }
}

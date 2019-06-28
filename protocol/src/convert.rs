use crate::cast;
use crate::protocol_generated::ckb::protocol as ckb_protocol;
use crate::FlatbuffersVectorIterator;
use ckb_core;
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::convert::{TryFrom, TryInto};

impl From<&H256> for ckb_protocol::H256 {
    fn from(h256: &H256) -> Self {
        let bytes = h256.as_fixed_bytes();
        Self::new(
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
            bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29], bytes[30], bytes[31],
        )
    }
}

impl TryFrom<&ckb_protocol::H256> for H256 {
    type Error = FailureError;

    fn try_from(h256: &ckb_protocol::H256) -> Result<Self, Self::Error> {
        let ret = H256::from_slice(&[
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
        ])?;
        Ok(ret)
    }
}

impl From<&ckb_core::transaction::ProposalShortId> for ckb_protocol::ProposalShortId {
    fn from(short_id: &ckb_core::transaction::ProposalShortId) -> Self {
        let bytes = *short_id;
        Self::new(
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9],
        )
    }
}

impl TryFrom<&ckb_protocol::ProposalShortId> for ckb_core::transaction::ProposalShortId {
    type Error = FailureError;

    fn try_from(short_id: &ckb_protocol::ProposalShortId) -> Result<Self, Self::Error> {
        let ret = cast!(Self::from_slice(&[
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
        ]))?;
        Ok(ret)
    }
}

impl<'a> TryFrom<ckb_protocol::Block<'a>> for ckb_core::block::Block {
    type Error = FailureError;

    fn try_from(block: ckb_protocol::Block<'a>) -> Result<Self, Self::Error> {
        let transactions: Result<Vec<ckb_core::transaction::Transaction>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(block.transactions())?)
                .map(TryInto::try_into)
                .collect();

        let uncles: Result<Vec<ckb_core::uncle::UncleBlock>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(block.uncles())?)
                .map(TryInto::try_into)
                .collect();

        let proposals: Result<Vec<ckb_core::transaction::ProposalShortId>, FailureError> =
            cast!(block.proposals())?
                .iter()
                .map(TryInto::try_into)
                .collect();

        let header = cast!(block.header())?;

        Ok(ckb_core::block::BlockBuilder::default()
            .header(TryInto::<ckb_core::header::Header>::try_into(header)?)
            .uncles(uncles?)
            .transactions(transactions?)
            .proposals(proposals?)
            .build())
    }
}

impl<'a> TryFrom<ckb_protocol::UncleBlock<'a>> for ckb_core::uncle::UncleBlock {
    type Error = FailureError;

    fn try_from(uncle_block: ckb_protocol::UncleBlock<'a>) -> Result<Self, Self::Error> {
        let proposals: Result<Vec<ckb_core::transaction::ProposalShortId>, FailureError> =
            cast!(uncle_block.proposals())?
                .iter()
                .map(TryInto::try_into)
                .collect();
        let header = cast!(uncle_block.header())?;

        Ok(ckb_core::uncle::UncleBlock {
            header: TryInto::try_into(header)?,
            proposals: proposals?,
        })
    }
}

impl<'a> TryFrom<ckb_protocol::Header<'a>> for ckb_core::header::Header {
    type Error = FailureError;

    fn try_from(header: ckb_protocol::Header<'a>) -> Result<Self, Self::Error> {
        let parent_hash = cast!(header.parent_hash())?;
        let transactions_root = cast!(header.transactions_root())?;
        let witnesses_root = cast!(header.witnesses_root())?;
        let proposals_hash = cast!(header.proposals_hash())?;
        let uncles_hash = cast!(header.uncles_hash())?;

        Ok(ckb_core::header::HeaderBuilder::default()
            .version(header.version())
            .parent_hash(TryInto::try_into(parent_hash)?)
            .timestamp(header.timestamp())
            .number(header.number())
            .epoch(header.epoch())
            .transactions_root(TryInto::try_into(transactions_root)?)
            .witnesses_root(TryInto::try_into(witnesses_root)?)
            .proposals_hash(TryInto::try_into(proposals_hash)?)
            .difficulty(U256::from_little_endian(cast!(header
                .difficulty()
                .and_then(|d| d.seq()))?)?)
            .uncles_hash(TryInto::try_into(uncles_hash)?)
            .nonce(header.nonce())
            .proof(cast!(header
                .proof()
                .and_then(|p| p.seq())
                .map(ckb_core::Bytes::from))?)
            .dao(cast!(header
                .dao()
                .and_then(|d| d.seq())
                .map(ckb_core::Bytes::from))?)
            .uncles_count(header.uncles_count())
            .build())
    }
}

impl<'a> TryFrom<ckb_protocol::Transaction<'a>> for ckb_core::transaction::Transaction {
    type Error = FailureError;

    fn try_from(transaction: ckb_protocol::Transaction<'a>) -> Result<Self, Self::Error> {
        let deps: Result<Vec<ckb_core::transaction::OutPoint>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(transaction.deps())?)
                .map(TryInto::try_into)
                .collect();

        let inputs: Result<Vec<ckb_core::transaction::CellInput>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(transaction.inputs())?)
                .map(TryInto::try_into)
                .collect();

        let outputs: Result<Vec<ckb_core::transaction::CellOutput>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(transaction.outputs())?)
                .map(TryInto::try_into)
                .collect();

        let witnesses: Result<Vec<ckb_core::transaction::Witness>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(transaction.witnesses())?)
                .map(TryInto::try_into)
                .collect();

        Ok(ckb_core::transaction::TransactionBuilder::default()
            .version(transaction.version())
            .deps(deps?)
            .inputs(inputs?)
            .outputs(outputs?)
            .witnesses(witnesses?)
            .build())
    }
}

impl<'a> TryFrom<ckb_protocol::RelayTransaction<'a>>
    for (ckb_core::transaction::Transaction, ckb_core::Cycle)
{
    type Error = FailureError;

    fn try_from(vtx: ckb_protocol::RelayTransaction<'a>) -> Result<Self, Self::Error> {
        let tx = cast!(vtx.transaction())?;
        let cycles = vtx.cycles();
        Ok((TryInto::try_into(tx)?, cycles))
    }
}

impl<'a> TryFrom<ckb_protocol::RelayTransactionHash<'a>> for H256 {
    type Error = FailureError;

    fn try_from(message: ckb_protocol::RelayTransactionHash<'a>) -> Result<Self, Self::Error> {
        let tx_hash = cast!(message.tx_hash())?;
        Ok(TryInto::try_into(tx_hash)?)
    }
}

impl<'a> TryFrom<ckb_protocol::GetRelayTransaction<'a>> for H256 {
    type Error = FailureError;

    fn try_from(message: ckb_protocol::GetRelayTransaction<'a>) -> Result<Self, Self::Error> {
        let tx_hash = cast!(message.tx_hash())?;
        Ok(TryInto::try_into(tx_hash)?)
    }
}

impl<'a> TryFrom<ckb_protocol::Witness<'a>> for ckb_core::transaction::Witness {
    type Error = FailureError;

    fn try_from(wit: ckb_protocol::Witness<'a>) -> Result<Self, Self::Error> {
        let data: Option<Vec<ckb_core::Bytes>> = FlatbuffersVectorIterator::new(cast!(wit.data())?)
            .map(|item| item.seq().map(ckb_core::Bytes::from))
            .collect();

        Ok(cast!(data)?)
    }
}

impl<'a> TryFrom<ckb_protocol::OutPoint<'a>> for ckb_core::transaction::OutPoint {
    type Error = FailureError;

    fn try_from(out_point: ckb_protocol::OutPoint<'a>) -> Result<Self, Self::Error> {
        let cell = match out_point.tx_hash() {
            Some(tx_hash) => Some(ckb_core::transaction::CellOutPoint {
                tx_hash: TryInto::try_into(tx_hash)?,
                index: out_point.index(),
            }),
            _ => None,
        };
        let block_hash = match out_point.block_hash() {
            Some(block_hash) => Some(TryInto::try_into(block_hash)?),
            None => None,
        };
        Ok(ckb_core::transaction::OutPoint { block_hash, cell })
    }
}

impl<'a> TryFrom<ckb_protocol::Script<'a>> for ckb_core::script::Script {
    type Error = FailureError;

    fn try_from(script: ckb_protocol::Script<'a>) -> Result<Self, Self::Error> {
        let args: Option<Vec<Vec<u8>>> = FlatbuffersVectorIterator::new(cast!(script.args())?)
            .map(|argument| argument.seq().map(|s| s.to_vec()))
            .collect();

        let code_hash = match script.code_hash() {
            Some(code_hash) => Some(TryInto::try_into(code_hash)?),
            None => None,
        };

        Ok(ckb_core::script::Script {
            args: cast!(args)?
                .into_iter()
                .map(ckb_core::Bytes::from)
                .collect(),
            code_hash: cast!(code_hash)?,
        })
    }
}

impl<'a> TryFrom<ckb_protocol::CellInput<'a>> for ckb_core::transaction::CellInput {
    type Error = FailureError;

    fn try_from(cell_input: ckb_protocol::CellInput<'a>) -> Result<Self, Self::Error> {
        let cell = match cell_input.tx_hash() {
            Some(tx_hash) => Some(ckb_core::transaction::CellOutPoint {
                tx_hash: TryInto::try_into(tx_hash)?,
                index: cell_input.index(),
            }),
            _ => None,
        };
        let block_hash = match cell_input.block_hash() {
            Some(block_hash) => Some(TryInto::try_into(block_hash)?),
            None => None,
        };
        let previous_output = ckb_core::transaction::OutPoint { block_hash, cell };

        Ok(ckb_core::transaction::CellInput {
            previous_output,
            since: cell_input.since(),
        })
    }
}

impl<'a> TryFrom<ckb_protocol::CellOutput<'a>> for ckb_core::transaction::CellOutput {
    type Error = FailureError;

    fn try_from(cell_output: ckb_protocol::CellOutput<'a>) -> Result<Self, Self::Error> {
        let lock = cast!(cell_output.lock())?;
        let type_ = match cell_output.type_() {
            Some(type_) => Some(TryInto::try_into(type_)?),
            None => None,
        };

        Ok(ckb_core::transaction::CellOutput {
            capacity: ckb_core::Capacity::shannons(cell_output.capacity()),
            data: ckb_core::Bytes::from(cast!(cell_output.data().and_then(|s| s.seq()))?),
            lock: TryInto::try_into(lock)?,
            type_,
        })
    }
}

impl<'a> TryFrom<ckb_protocol::IndexTransaction<'a>> for ckb_core::transaction::IndexTransaction {
    type Error = FailureError;

    fn try_from(it: ckb_protocol::IndexTransaction<'a>) -> Result<Self, Self::Error> {
        let transaction = cast!(it.transaction())?;
        Ok(ckb_core::transaction::IndexTransaction {
            index: it.index() as usize,
            transaction: TryInto::try_into(transaction)?,
        })
    }
}

impl<'a> TryFrom<ckb_protocol::Alert<'a>> for ckb_core::alert::Alert {
    type Error = FailureError;

    fn try_from(alert: ckb_protocol::Alert<'a>) -> Result<Self, Self::Error> {
        let message = String::from_utf8(cast!(alert.message().and_then(|m| m.seq()))?.to_owned())?;
        let min_version: Option<String> = match alert
            .min_version()
            .and_then(|m| m.seq().map(|s| String::from_utf8(s.to_vec())))
        {
            Some(min_version) => Some(min_version?),
            None => None,
        };
        let max_version: Option<String> = match alert
            .max_version()
            .and_then(|m| m.seq().map(|s| String::from_utf8(s.to_vec())))
        {
            Some(max_version) => Some(max_version?),
            None => None,
        };
        let signatures: Result<Vec<ckb_core::Bytes>, FailureError> =
            FlatbuffersVectorIterator::new(cast!(alert.signatures())?)
                .map(|s| {
                    cast!(s.seq())
                        .map(ckb_core::Bytes::from)
                        .map_err(Into::into)
                })
                .collect();
        Ok(ckb_core::alert::AlertBuilder::default()
            .id(alert.id())
            .cancel(alert.cancel())
            .min_version(min_version)
            .max_version(max_version)
            .priority(alert.priority())
            .signatures(signatures?)
            .notice_until(alert.notice_until())
            .message(message)
            .build())
    }
}

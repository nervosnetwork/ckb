use crate::block::{cal_proposals_hash, Block};
use crate::header::Header;
use crate::transaction::ProposalShortId;
use crate::BlockNumber;
use canonical_serializer::{CanonicalSerialize, CanonicalSerializer, Result as SerializeResult};
use hash::Blake2bWriter;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::io::Write;

#[derive(Clone, Serialize, Deserialize, Eq, Debug)]
pub struct UncleBlock {
    pub header: Header,
    pub proposals: Vec<ProposalShortId>,
}

impl From<Block> for UncleBlock {
    fn from(block: Block) -> Self {
        UncleBlock {
            header: block.header().to_owned(),
            proposals: block.proposals().to_vec(),
        }
    }
}

impl CanonicalSerialize for UncleBlock {
    fn serialize<W: Write>(&self, serializer: &mut CanonicalSerializer<W>) -> SerializeResult<()> {
        serializer
            .encode_struct_ref(&self.header)?
            .encode_vec(&self.proposals)?;
        Ok(())
    }
}

impl UncleBlock {
    pub fn new(header: Header, proposals: Vec<ProposalShortId>) -> UncleBlock {
        UncleBlock { header, proposals }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn hash(&self) -> &H256 {
        self.header.hash()
    }

    pub fn number(&self) -> BlockNumber {
        self.header.number()
    }

    pub fn proposals(&self) -> &[ProposalShortId] {
        &self.proposals
    }

    pub fn cal_proposals_hash(&self) -> H256 {
        cal_proposals_hash(self.proposals())
    }

    pub fn serialized_size(&self, proof_size: usize) -> usize {
        Header::serialized_size(proof_size)
    }
}

impl PartialEq for UncleBlock {
    fn eq(&self, other: &UncleBlock) -> bool {
        self.header().hash() == other.header().hash()
    }
}

impl ::std::hash::Hash for UncleBlock {
    fn hash<H>(&self, state: &mut H)
    where
        H: ::std::hash::Hasher,
    {
        state.write(&self.header.hash().as_bytes());
        state.finish();
    }
}

pub fn uncles_hash(uncles: &[UncleBlock]) -> H256 {
    if uncles.is_empty() {
        H256::zero()
    } else {
        let mut hasher = Blake2bWriter::new();
        let mut serializer = CanonicalSerializer::new(&mut hasher);
        serializer
            .encode_vec(uncles)
            .expect("Uncles canonical serialize");
        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockBuilder;

    #[test]
    fn block_size_should_not_include_uncles_proposal_zones() {
        let uncle1: UncleBlock = BlockBuilder::default()
            .proposal(ProposalShortId::zero())
            .build()
            .into();
        let uncle2: UncleBlock = BlockBuilder::default().build().into();

        assert_eq!(uncle1.serialized_size(0), uncle2.serialized_size(0));
    }
}

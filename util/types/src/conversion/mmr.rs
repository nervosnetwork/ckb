use crate::{packed, prelude::*, utilities::merkle_mountain_range::MMRProof};

impl Pack<packed::BlockProof> for MMRProof {
    fn pack(&self) -> packed::BlockProof {
        packed::BlockProof::new_builder()
            .mmr_size(self.mmr_size().pack())
            .items(
                self.proof_items()
                    .iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
                    .pack(),
            )
            .build()
    }
}

impl<'r> Unpack<MMRProof> for packed::BlockProofReader<'r> {
    fn unpack(&self) -> MMRProof {
        let mmr_size: u64 = self.mmr_size().unpack();
        let proof = self
            .items()
            .iter()
            .map(|item| item.to_entity())
            .collect::<Vec<_>>();
        MMRProof::new(mmr_size, proof)
    }
}
impl_conversion_for_entity_unpack!(MMRProof, BlockProof);

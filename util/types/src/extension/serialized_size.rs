use crate::{packed, prelude::*};

macro_rules! impl_serialized_size_for_entity {
    ($entity:ident, $func:ident) => {
        impl packed::$entity {
            /// TODO(doc): @yangby-cryptape
            pub fn $func(&self) -> usize {
                self.as_reader().$func()
            }
        }
    };
}

impl<'r> packed::TransactionReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn serialized_size_in_block(&self) -> usize {
        // the offset in TransactionVec header is u32
        self.as_slice().len() + molecule::NUMBER_SIZE
    }
}
impl_serialized_size_for_entity!(Transaction, serialized_size_in_block);

impl<'r> packed::BlockReader<'r> {
    /// TODO(doc): @yangby-cryptape
    pub fn serialized_size_without_uncle_proposals(&self) -> usize {
        let block_size = self.as_slice().len();
        // the header of ProposalShortIdVec header is u32
        let uncles_proposals_size = self
            .uncles()
            .iter()
            .map(|x| x.proposals().as_slice().len() - molecule::NUMBER_SIZE)
            .sum::<usize>();
        block_size - uncles_proposals_size
    }
}
impl_serialized_size_for_entity!(Block, serialized_size_without_uncle_proposals);

#[cfg(test)]
mod tests {
    use crate::{packed, prelude::*};

    #[test]
    fn block_size_should_not_include_uncles_proposals() {
        let proposal1 = [1; 10].pack();
        let proposal2 = [2; 10].pack();
        let proposal3 = [3; 10].pack();
        let proposals1 = vec![proposal1.clone()].pack();
        let proposals2 = vec![proposal1.clone(), proposal2.clone()].pack();
        let proposals3 = vec![proposal1, proposal2, proposal3].pack();
        let uncle0 = packed::UncleBlock::new_builder().build();
        let uncle1 = packed::UncleBlock::new_builder()
            .proposals(proposals1)
            .build();
        let uncle2 = packed::UncleBlock::new_builder()
            .proposals(proposals2)
            .build();
        let uncle3 = packed::UncleBlock::new_builder()
            .proposals(proposals3)
            .build();
        let mut empty_uncles = vec![
            uncle0.clone(),
            uncle0.clone(),
            uncle0.clone(),
            uncle0.clone(),
        ];
        let mut uncles = vec![uncle0, uncle1, uncle2, uncle3];
        loop {
            let block_with_empty_uncles = packed::Block::new_builder()
                .uncles(empty_uncles.clone().pack())
                .build();
            let block_with_uncles = packed::Block::new_builder()
                .uncles(uncles.clone().pack())
                .build();
            let actual = block_with_uncles.serialized_size_without_uncle_proposals();
            let actual_empty = block_with_empty_uncles.serialized_size_without_uncle_proposals();
            let expected = block_with_empty_uncles.as_slice().len();
            assert_eq!(actual, actual_empty);
            assert_eq!(actual, expected);
            if uncles.is_empty() {
                break;
            } else {
                empty_uncles.pop();
                uncles.pop();
            }
        }
    }
}

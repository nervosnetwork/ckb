use crate::{packed, prelude::*};

macro_rules! impl_serialized_size_for_entity {
    ($entity:ident, $func:ident, $reader_func_link:expr) => {
        impl packed::$entity {
            /// Calls
            #[doc = $reader_func_link]
            pub fn $func(&self) -> usize {
                self.as_reader().$func()
            }
        }
    };
    ($entity:ident, $func:ident) => {
        impl_serialized_size_for_entity!(
            $entity,
            $func,
            concat!(
                "[`",
                stringify!($entity),
                "::",
                stringify!($func),
                "(..)`](struct.",
                stringify!($entity),
                "Reader.html#method.",
                stringify!($func),
                ")."
            )
        );
    };
}

impl<'r> packed::TransactionReader<'r> {
    /// Calculates the serialized size of a [`Transaction`] in [`Block`].
    ///
    /// Put each [`Transaction`] into [`Block`] will occupy extra spaces to store [an offset in header],
    /// its size is [`molecule::NUMBER_SIZE`].
    ///
    /// [`Transaction`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L66-L69
    /// [`Block`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L94-L99
    /// [an offset in header]: https://github.com/nervosnetwork/molecule/blob/df1fdce/docs/encoding_spec.md#memory-layout
    /// [`molecule::NUMBER_SIZE`]: https://docs.rs/molecule/0.6.1/molecule/constant.NUMBER_SIZE.html
    pub fn serialized_size_in_block(&self) -> usize {
        self.as_slice().len() + molecule::NUMBER_SIZE
    }
}
impl_serialized_size_for_entity!(Transaction, serialized_size_in_block);

impl<'r> packed::BlockReader<'r> {
    /// Calculates the serialized size of [`Block`] without [uncle proposals].
    ///
    /// # Computational Steps
    /// - Calculates the total serialized size of [`Block`], marks it as `B`.
    /// - Calculates the serialized size [`ProposalShortIdVec`] for each uncle block, marks them as
    ///   `P0, P1, ..., Pn`.
    /// - Even an uncle has no proposals, the [`ProposalShortIdVec`] still has [a header contains its total size],
    ///   the size is [`molecule::NUMBER_SIZE`], marks it as `h`.
    /// - So the serialized size of [`Block`] without [uncle proposals] is: `B - sum(P0 - h, P1 - h, ..., Pn - h)`
    ///
    /// [`Block`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L94-L99
    /// [uncle proposals]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L91
    /// [`ProposalShortIdVec`]: https://github.com/nervosnetwork/ckb/blob/v0.36.0/util/types/schemas/blockchain.mol#L25
    /// [a header contains its total size]: https://github.com/nervosnetwork/molecule/blob/df1fdce/docs/encoding_spec.md#memory-layout
    /// [`molecule::NUMBER_SIZE`]: https://docs.rs/molecule/0.6.1/molecule/constant.NUMBER_SIZE.html
    pub fn serialized_size_without_uncle_proposals(&self) -> usize {
        let block_size = self.as_slice().len();
        let uncles_proposals_size = self
            .uncles()
            .iter()
            .map(|x| x.proposals().as_slice().len() - molecule::NUMBER_SIZE)
            .sum::<usize>();
        block_size - uncles_proposals_size
    }
}
impl_serialized_size_for_entity!(Block, serialized_size_without_uncle_proposals);

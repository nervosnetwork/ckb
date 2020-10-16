use crate::{core::Capacity, packed::Byte32};

/// Details of miner rewards issued by block cellbase transaction.
///
/// # References:
/// - [Token Issuance](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
/// - [Miner Compensation](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#miner-compensation)
/// - [Paying for Transaction Fees](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#paying-for-transaction-fees)
/// - [`RewardCalculator::txs_fee(..)`](../../ckb_reward_calculator/struct.RewardCalculator.html#method.txs_fees)
/// - [Collecting State Rent with Secondary Issuance and the NervosDAO](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#collecting-state-rent-with-secondary-issuance-and-the-nervosdao)
#[derive(Debug, Default)]
pub struct BlockReward {
    /// The total block reward.
    pub total: Capacity,
    /// The primary block reward.
    pub primary: Capacity,
    /// The secondary block reward.
    ///
    /// # Notice
    /// Only 60% of the secondary issuance goes to the miners, 35% of the issuance goes to the NervosDAO
    /// and the last 5% are kept liquid.
    pub secondary: Capacity,
    /// The transaction fees that are rewarded to miners because the transaction is committed in
    /// the block.
    ///
    /// # Notice
    /// Miners only get 60% of the transaction fee for each transaction committed in the block.
    pub tx_fee: Capacity,
    /// The transaction fees that are rewarded to miners because the transaction is proposed in the
    /// block or its uncles.
    ///
    /// # Notice
    /// Miners only get 40% of the transaction fee for each transaction proposed in the block
    /// and committed later in its active commit window.
    pub proposal_reward: Capacity,
}

/// Native token issuance.
///
/// # References:
/// - [Token Issuance](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockIssuance {
    /// The primary issuance.
    pub primary: Capacity,
    /// The secondary issuance.
    pub secondary: Capacity,
}

/// Miner reward.
///
/// # References:
/// - [Token Issuance](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#token-issuance)
/// - [Miner Compensation](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#miner-compensation)
/// - [Paying for Transaction Fees](https://github.com/nervosnetwork/rfcs/blob/v2020.01.15/rfcs/0015-ckb-cryptoeconomics/0015-ckb-cryptoeconomics.md#paying-for-transaction-fees)
/// - [`RewardCalculator::txs_fee(..)`](../../ckb_reward_calculator/struct.RewardCalculator.html#method.txs_fees)
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MinerReward {
    /// The miner receives all the primary issuance.
    pub primary: Capacity,
    /// The miner receives part of the secondary issuance.
    pub secondary: Capacity,
    /// The miner recevies 60% of the transaction fee for each transaction committed in the block.
    pub committed: Capacity,
    /// The miner recevies 40% of the transaction fee for each transaction proposed in the block,
    /// and committed later in its active commit window.
    pub proposal: Capacity,
}

/// Includes the rewards details for a block and when the block is finalized.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockEconomicState {
    /// Native token issuance in the block.
    pub issuance: BlockIssuance,
    /// Miner reward in the block.
    pub miner_reward: MinerReward,
    /// The total fees of all transactions committed in the block.
    pub txs_fee: Capacity,
    ///  The block hash of the block which creates the rewards as cells in its cellbase
    ///  transaction.
    pub finalized_at: Byte32,
}

impl From<BlockReward> for MinerReward {
    fn from(reward: BlockReward) -> Self {
        Self {
            primary: reward.primary,
            secondary: reward.secondary,
            committed: reward.tx_fee,
            proposal: reward.proposal_reward,
        }
    }
}

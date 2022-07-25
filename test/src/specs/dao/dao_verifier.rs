use crate::Node;
use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::extract_dao_data;
use ckb_jsonrpc_types::EpochView;
use ckb_types::core::{BlockEconomicState, BlockNumber, BlockView, Capacity, TransactionView};
use ckb_types::packed::{Byte32, CellOutput, OutPoint};
use ckb_types::prelude::Unpack;
use ckb_util::Mutex;
use std::collections::HashMap;

#[derive(Default)]
#[allow(non_snake_case)]
pub struct DAOVerifier {
    consensus: Consensus,
    tip_number: BlockNumber,
    blocks: Vec<BlockView>,
    transactions: HashMap<Byte32, (BlockNumber, TransactionView)>,
    epochs: Vec<EpochView>,
    blocks_reward: Vec<Option<BlockEconomicState>>,

    cache_C: Mutex<HashMap<BlockNumber, u64>>,
    cache_S: Mutex<HashMap<BlockNumber, u64>>,
    cache_U: Mutex<HashMap<BlockNumber, u64>>,
    cache_ar: Mutex<HashMap<BlockNumber, u64>>,
}

impl DAOVerifier {
    pub fn init(node: &Node) -> Self {
        let consensus = node.consensus().clone();
        let tip_number = node.get_tip_block_number();
        let mut blocks = Vec::new();
        let mut transactions = HashMap::new();
        let mut epochs = Vec::new();
        let mut blocks_reward = Vec::new();
        for number in 0..=node.get_tip_block_number() {
            blocks.push(node.get_block_by_number(number))
        }
        for block in blocks.iter() {
            for transaction in block.transactions() {
                transactions.insert(transaction.hash(), (block.number(), transaction));
            }
        }
        for number in 0..=node.rpc_client().get_current_epoch().number.value() {
            epochs.push(node.rpc_client().get_epoch_by_number(number).unwrap());
        }
        for block in blocks.iter() {
            blocks_reward.push(
                node.rpc_client()
                    .get_block_economic_state(block.hash())
                    .map(Into::into),
            );
        }
        Self {
            consensus,
            tip_number,
            blocks,
            transactions,
            epochs,
            blocks_reward,
            ..Default::default()
        }
    }

    pub fn ar0(&self) -> u64 {
        let ar0 = 10u64.pow(16);
        self.cache_ar.lock().insert(0, ar0);
        ar0
    }

    pub fn p(&self, i: BlockNumber) -> u64 {
        for epoch in self.epochs.iter() {
            if epoch.start_number.value() <= i
                && i < epoch.start_number.value() + epoch.length.value()
            {
                let epoch_primary_reward =
                    self.consensus.primary_epoch_reward(epoch.number.value());
                if i - epoch.start_number.value()
                    < epoch_primary_reward.as_u64() % epoch.length.value()
                {
                    return epoch_primary_reward.as_u64() / epoch.length.value() + 1;
                } else {
                    return epoch_primary_reward.as_u64() / epoch.length.value();
                }
            }
        }
        unreachable!()
    }

    pub fn s(&self, i: BlockNumber) -> u64 {
        for epoch in self.epochs.iter() {
            if epoch.start_number.value() <= i
                && i < epoch.start_number.value() + epoch.length.value()
            {
                let epoch_secondary_reward = self.consensus.secondary_epoch_reward();
                if i - epoch.start_number.value()
                    < epoch_secondary_reward.as_u64() % epoch.length.value()
                {
                    return epoch_secondary_reward.as_u64() / epoch.length.value() + 1;
                } else {
                    return epoch_secondary_reward.as_u64() / epoch.length.value();
                }
            }
        }
        unreachable!()
    }

    pub fn ar(&self, i: BlockNumber) -> u64 {
        {
            if let Some(ar) = self.cache_ar.lock().get(&i) {
                return *ar;
            }
        }
        if i == 0 {
            return self.ar0();
        }

        let ar = self.ar(i - 1)
            + u64::try_from(
                u128::from(self.ar(i - 1)) * u128::from(self.s(i)) / u128::from(self.C(i - 1)),
            )
            .unwrap();
        self.cache_ar.lock().insert(i, ar);
        ar
    }

    #[allow(non_snake_case)]
    pub fn U_in(&self, i: BlockNumber) -> u64 {
        let mut sum = 0u64;
        for tx in self.blocks[i as usize].transactions() {
            for o in tx.input_pts_iter() {
                if !o.is_null() {
                    sum += self.get_output_occupied_capacity(&o);
                }
            }
        }
        sum
    }

    #[allow(non_snake_case)]
    pub fn U_out(&self, i: BlockNumber) -> u64 {
        let satoshi_cell_occupied_ratio = self.consensus.satoshi_cell_occupied_ratio;
        let satoshi_pubkey_hash = &self.consensus.satoshi_pubkey_hash;
        let mut sum = 0u64;
        for (tx_index, tx) in self.blocks[i as usize].transactions().iter().enumerate() {
            for (out_point, output) in tx.output_pts().iter().zip(tx.outputs().into_iter()) {
                if i == 0
                    && tx_index == 0
                    && output.lock().args().raw_data() == satoshi_pubkey_hash.0[..]
                {
                    sum += Unpack::<Capacity>::unpack(&output.capacity())
                        .safe_mul_ratio(satoshi_cell_occupied_ratio)
                        .unwrap()
                        .as_u64();
                } else {
                    sum += self.get_output_occupied_capacity(out_point);
                }
            }
        }
        sum
    }

    #[allow(non_snake_case)]
    pub fn C_in(&self, i: BlockNumber) -> u64 {
        let mut sum = 0u64;
        for tx in self.blocks[i as usize].transactions() {
            for o in tx.input_pts_iter() {
                if !o.is_null() {
                    sum += self.get_output_capacity(&o);
                }
            }
        }
        sum
    }

    #[allow(non_snake_case)]
    pub fn C_out(&self, i: BlockNumber) -> u64 {
        let mut sum = 0u64;
        for tx in self.blocks[i as usize].transactions() {
            for o in tx.output_pts() {
                sum += self.get_output_capacity(&o);
            }
        }
        sum
    }

    #[allow(non_snake_case)]
    pub fn C0(&self) -> u64 {
        let C0 = self.C_out(0) - self.C_in(0) + self.p(0) + self.s(0);
        self.cache_C.lock().insert(0, C0);
        C0
    }

    #[allow(non_snake_case)]
    pub fn U0(&self) -> u64 {
        let U0 = self.U_out(0) - self.U_in(0);
        self.cache_U.lock().insert(0, U0);
        U0
    }

    #[allow(non_snake_case)]
    pub fn S0(&self) -> u64 {
        let S0 = self.s(0);
        self.cache_S.lock().insert(0, S0);
        S0
    }

    #[allow(non_snake_case)]
    pub fn I(&self, i: BlockNumber) -> u64 {
        let mut sum = 0u64;
        for tx in self.blocks[i as usize].transactions() {
            for o in tx.input_pts_iter() {
                if o.is_null() {
                    continue;
                }

                if self.is_dao_prepare_input(&o) {
                    // `o` is prepare point, then `o`'s same-position input is deposit point
                    let prepare_tx = self.get_transaction(&o.tx_hash());
                    let deposit_out_point = prepare_tx
                        .inputs()
                        .get(o.index().unpack())
                        .unwrap()
                        .previous_output();
                    let deposit_header_number =
                        self.get_tx_block_number(&deposit_out_point.tx_hash());
                    let prepare_header_number = self.get_tx_block_number(&prepare_tx.hash());
                    let deposit_ar = self.ar(deposit_header_number);
                    let prepare_ar = self.ar(prepare_header_number);
                    let deposit_counted_capacity = self.get_output_capacity(&deposit_out_point)
                        - self.get_output_occupied_capacity(&o);
                    let prepare_capacity = u64::try_from(
                        u128::from(deposit_counted_capacity) * u128::from(prepare_ar)
                            / u128::from(deposit_ar),
                    )
                    .unwrap();
                    let interest = prepare_capacity - deposit_counted_capacity;
                    sum += interest
                }
            }
        }
        sum
    }

    fn is_dao_prepare_input(&self, out_point: &OutPoint) -> bool {
        let input_tx = self.get_transaction(&out_point.tx_hash());
        let input_data = input_tx
            .outputs_data()
            .get(out_point.index().unpack())
            .unwrap();
        if input_data.len() != 8 {
            return false;
        }

        let deposited_number = LittleEndian::read_u64(&input_data.raw_data()[0..8]);
        if deposited_number == 0 {
            return false;
        }

        let dao_type_hash = self.consensus.dao_type_hash().unwrap();
        self.get_output(out_point)
            .type_()
            .to_opt()
            .map(|script| script.code_hash() == dao_type_hash)
            .unwrap_or(false)
    }

    #[allow(non_snake_case)]
    pub fn C(&self, i: BlockNumber) -> u64 {
        {
            if let Some(C) = self.cache_C.lock().get(&i) {
                return *C;
            }
        }
        if i == 0 {
            return self.C0();
        }

        let C = self.C(i - 1) + self.p(i) + self.s(i);
        self.cache_C.lock().insert(i, C);
        C
    }

    #[allow(non_snake_case)]
    pub fn S(&self, i: BlockNumber) -> u64 {
        {
            if let Some(S) = self.cache_S.lock().get(&i) {
                return *S;
            }
        }
        if i == 0 {
            return self.S0();
        }

        let S = self.S(i - 1) - self.I(i) + self.s(i)
            - u64::try_from(
                u128::from(self.s(i)) * u128::from(self.U(i - 1)) / u128::from(self.C(i - 1)),
            )
            .unwrap();
        self.cache_S.lock().insert(i, S);
        S
    }

    #[allow(non_snake_case)]
    pub fn U(&self, i: BlockNumber) -> u64 {
        {
            if let Some(U) = self.cache_U.lock().get(&i) {
                return *U;
            }
        }
        if i == 0 {
            return self.U0();
        }

        let U = self.U(i - 1) + self.U_out(i) - self.U_in(i);
        self.cache_U.lock().insert(i, U);
        U
    }

    fn get_tx_block_number(&self, tx_hash: &Byte32) -> BlockNumber {
        self.transactions
            .get(tx_hash)
            .map(|(i, _)| *i)
            .expect("exist")
    }

    fn get_transaction(&self, tx_hash: &Byte32) -> &TransactionView {
        self.transactions
            .get(tx_hash)
            .map(|(_, tx)| tx)
            .expect("exist")
    }

    fn get_output(&self, out_point: &OutPoint) -> CellOutput {
        self.get_transaction(&out_point.tx_hash())
            .output(out_point.index().unpack())
            .expect("exist")
    }

    fn get_output_capacity(&self, out_point: &OutPoint) -> u64 {
        self.get_output(out_point).capacity().unpack()
    }

    fn get_output_occupied_capacity(&self, out_point: &OutPoint) -> u64 {
        let satoshi_pubkey_hash = &self.consensus.satoshi_pubkey_hash;
        let satoshi_cell_occupied_ratio = self.consensus.satoshi_cell_occupied_ratio;
        let (output, data) = self
            .get_transaction(&out_point.tx_hash())
            .output_with_data(out_point.index().unpack())
            .expect("exist");
        if Unpack::<u32>::unpack(&out_point.index()) == 0
            && output.lock().args().raw_data() == satoshi_pubkey_hash.0[..]
        {
            Unpack::<Capacity>::unpack(&output.capacity())
                .safe_mul_ratio(satoshi_cell_occupied_ratio)
                .unwrap()
                .as_u64()
        } else {
            output
                .occupied_capacity(Capacity::bytes(data.len()).unwrap())
                .unwrap()
                .as_u64()
        }
    }

    pub fn verify(&self) {
        self.blocks.iter().for_each(|block| {
            assert_eq!(
                self.C(block.number()),
                extract_dao_data(block.dao()).1.as_u64(),
                "assert C. expected_dao_field: {}",
                self.expected_dao_field(block.number()),
            );
        });
        self.blocks.iter().for_each(|block| {
            assert_eq!(
                self.U(block.number()),
                extract_dao_data(block.dao()).3.as_u64(),
                "assert U. expected_dao_field: {}",
                self.expected_dao_field(block.number()),
            );
        });
        self.blocks.iter().for_each(|block| {
            let finalization_delay_length = self.consensus.finalization_delay_length();
            let i = block.number();
            let reward = &self.blocks_reward[i as usize];
            if i == 0 || i + finalization_delay_length > self.tip_number {
                assert!(
                    reward.is_none(),
                    "assert block_reward_{}. Non finalized block should has not economic state. actual: {:?}",
                    i,
                    reward,
                );
            } else {
                assert_eq!(
                    Some(self.p(i)),
                    reward.as_ref().map(|reward| reward.issuance.primary.as_u64()),
                    "assert block_reward_{}",
                    i,
                );
            }
        });
        self.blocks.iter().for_each(|block| {
            assert_eq!(
                self.ar(block.number()),
                extract_dao_data(block.dao()).0,
                "assert ar. expected_dao_field: {}",
                self.expected_dao_field(block.number()),
            );
        });
        self.blocks.iter().for_each(|block| {
            assert_eq!(
                self.S(block.number()),
                extract_dao_data(block.dao()).2.as_u64(),
                "assert S. expected_dao_field: {}",
                self.expected_dao_field(block.number()),
            );
        });
    }

    fn expected_dao_field(&self, i: BlockNumber) -> String {
        format!(
            "C_{}: {}, S_{}: {}, U_{}: {}, ar_{}: {}",
            i,
            self.C(i),
            i,
            self.S(i),
            i,
            self.U(i),
            i,
            self.ar(i),
        )
    }
}

use crate::{Net, Node, Spec, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_chain_spec::ChainSpec;
use ckb_jsonrpc_types::{BlockTemplate, JsonBytes};
use ckb_types::{core::BlockView, packed::{ProposalShortId, POAWitness, Byte65, Byte32, CellbaseExtWitness, CellbaseWitness}, prelude::*, bytes::Bytes, utilities::{merkle_root, CBMT}};
use ckb_pow::{Pow, POAEngineConfig};
use ckb_crypto::secp::{Generator, Privkey, Message, Signature};
use ckb_hash::blake2b_256;
use log::info;
use std::convert::Into;
use std::thread::sleep;
use std::time::Duration;

pub struct POAMining {
    pubkey_hash: Bytes,
    privkey: Privkey,
}

impl Spec for POAMining {
    crate::name!("poa_mining");

    fn run(&self, net: &mut Net) {
        let node = &net.nodes[0];
        self.test_basic(node);
        self.reject_dummy_block(node);
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        let pubkey_hash = JsonBytes::from_bytes(self.pubkey_hash.clone());
        Box::new(move |spec_config| {
            spec_config.pow = Pow::POA(POAEngineConfig{ pubkey_hash: pubkey_hash.clone() });
        })
    }
}

impl POAMining {
    pub fn new() -> Self {
        let mut generator = Generator::new();
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let pubkey_hash = Bytes::from(&blake2b_256(&pubkey_data)[0..20]);
        POAMining {
            pubkey_hash,
            privkey
        }
    }

    pub fn test_basic(&self, node: &Node) {
        for _ in 0..(DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) {
            generate_poa_block(node, &self.privkey);
        }
        info!("Use generated block's cellbase as tx input");
        let transaction_hash = node.generate_transaction();
        let block1_hash = 
            generate_poa_block(node, &self.privkey);
        let _ = generate_poa_block(node, &self.privkey); // skip
        let block3_hash = generate_poa_block(node, &self.privkey);

        let block1: BlockView = node.rpc_client().get_block(block1_hash).unwrap().into();
        let block3: BlockView = node.rpc_client().get_block(block3_hash).unwrap().into();

        info!("Generated tx should be included in next block's proposal txs");
        assert!(block1
            .union_proposal_ids_iter()
            .any(|id| ProposalShortId::from_tx_hash(&transaction_hash).eq(&id)));

        info!("Generated tx should be included in next + n block's commit txs, current n = 2");
        assert!(block3
            .transactions()
            .into_iter()
            .any(|tx| transaction_hash.eq(&tx.hash())));
    }

    pub fn reject_dummy_block(&self, node: &Node){
        let block = node.new_block(None, None, None);
        let result = node.rpc_client()
            .submit_block("".to_owned(), block.data().into());
        info!("Dummy block should be rejected");
        assert!(result.is_err());
    }
}

// generate a new poa block and submit it through rpc.
// 1. sign block hash
// 2. construct POAWitness
// 3. reconstruct cellbase and block
pub fn generate_poa_block(node: &Node, privkey: &Privkey) -> Byte32 {
    let block = node.new_block(None, None, None);
    let raw_tx_root = merkle_root(&block.transactions().into_iter().map(|tx|tx.hash()).collect::<Vec<_>>());
    let proof = CBMT::build_merkle_proof(&block.transactions().into_iter().map(|tx|tx.witness_hash()).collect::<Vec<_>>(), &[0]).unwrap();
    let poa_witness = POAWitness::new_builder().witnesses_root_proof(proof.lemmas().to_vec().pack()).raw_transactions_root(raw_tx_root.clone()).transactions_count((block.transactions().len() as u32).pack()).build();
    let cellbase_without_signature = {
        let cellbase = block.transactions()[0].data();
        let cellbase_witness: Bytes = cellbase.witnesses().get(0).unwrap().unpack();
        let witness = CellbaseExtWitness::from_slice(&cellbase_witness).unwrap().as_builder().extension(poa_witness.as_bytes().pack()).build();
        cellbase.as_advanced_builder().set_witnesses(vec![witness.as_bytes().pack()]).build()
    };
    let message : Message = {
        let mut txs = vec![cellbase_without_signature];
        txs.extend(block.transactions().into_iter().skip(1));
        let witness_root = merkle_root(&txs.iter().map(|tx| tx.witness_hash()).collect::<Vec<_>>());
        let tx_root = merkle_root(&[raw_tx_root.clone(), witness_root]);
    let block = block.as_advanced_builder().set_transactions(txs).transactions_root(tx_root).build();
        let block_hash: [u8;32] = block.hash().unpack();
        block_hash.into()
    };
    let signature = privkey.sign_recoverable(&message).unwrap();
    let poa_witness = 
        poa_witness.as_builder().signature(Byte65::new_unchecked(signature.serialize().into())).build()
    ;
    let new_cellbase = {
        let cellbase = block.transactions()[0].data();
        let cellbase_witness: Bytes = cellbase.witnesses().get(0).unwrap().unpack();
        let witness = CellbaseExtWitness::from_slice(&cellbase_witness).unwrap().as_builder().extension(poa_witness.as_bytes().pack()).build();
        cellbase.as_advanced_builder().set_witnesses(vec![witness.as_bytes().pack()]).build()
    };
    let mut txs = vec![new_cellbase];
    txs.extend(block.transactions().into_iter().skip(1));
    let witness_root = merkle_root(&txs.iter().map(|tx| tx.witness_hash()).collect::<Vec<_>>());
    let tx_root = merkle_root(&[raw_tx_root, witness_root]);
    let block = block.as_advanced_builder().set_transactions(txs).transactions_root(tx_root).build();
    node.submit_block(&block)
}

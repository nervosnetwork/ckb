use crate::specs::dao::dao_verifier::DAOVerifier;
use crate::util::mining::mine;
use crate::{Node, Spec};

pub struct DAOVerify;

impl Spec for DAOVerify {
    fn modify_chain_spec(&self, spec: &mut ckb_chain_spec::ChainSpec) {
        spec.params.genesis_epoch_length = 20;
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        let node = &nodes[0];
        let genesis_epoch_length = node.consensus().genesis_epoch_ext().length();
        mine(node, genesis_epoch_length * 5);
        DAOVerifier::init(node).verify();
    }
}

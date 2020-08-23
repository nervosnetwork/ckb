use crate::specs::dao::dao_verifier::DAOVerifier;
use crate::{Net, Spec};
use ckb_chain_spec::ChainSpec;

pub struct DAOVerify;

impl Spec for DAOVerify {
    crate::name!("dao_verify");

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec)> {
        Box::new(|spec_config| {
            spec_config.params.genesis_epoch_length = 20;
        })
    }

    fn run(&self, net: &mut Net) {
        let node = net.node(0);
        let genesis_epoch_length = node.consensus().genesis_epoch_ext().length();
        node.generate_blocks(genesis_epoch_length as usize * 5);
        DAOVerifier::init(node).verify();
    }
}

use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_protocol;
use network::NetworkContextExt;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct GetDataProcess<'a, C: 'a> {
    message: &'a ckb_protocol::GetData,
    synchronizer: &'a Synchronizer<C>,
    nc: &'a NetworkContext,
}

impl<'a, C> GetDataProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a ckb_protocol::GetData,
        synchronizer: &'a Synchronizer<C>,
        _peer: &PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        GetDataProcess {
            message,
            nc,
            synchronizer,
        }
    }

    pub fn execute(self) {
        let inventory_vec = self.message.get_inventory();
        for inventory in inventory_vec.iter() {
            debug!(target: "sync", "inv {:?}", H256::from(inventory.get_hash()));
            InventoryProcess::new(self.nc, self.synchronizer, inventory).execute();
        }
    }
}

pub struct InventoryProcess<'a, C: 'a> {
    nc: &'a NetworkContext,
    synchronizer: &'a Synchronizer<C>,
    inventory: &'a ckb_protocol::Inventory,
}

impl<'a, C> InventoryProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        nc: &'a NetworkContext,
        synchronizer: &'a Synchronizer<C>,
        inventory: &'a ckb_protocol::Inventory,
    ) -> Self {
        InventoryProcess {
            nc,
            inventory,
            synchronizer,
        }
    }

    pub fn execute(self) {
        let inv_type = self.inventory.get_inv_type();
        match inv_type {
            ckb_protocol::InventoryType::MSG_BLOCK => {
                if let Some(ref block) = self
                    .synchronizer
                    .get_block(&H256::from(self.inventory.get_hash()))
                {
                    let mut payload = ckb_protocol::Payload::new();
                    debug!(target: "sync", "respond_block {} {:?}", block.number(), block.hash());
                    payload.set_block(block.into());
                    let _ = self.nc.respond_payload(payload);
                } else {
                    //Reponse notfound
                }
            }
            ckb_protocol::InventoryType::ERROR => {}
            _ => {}
        }
    }
}

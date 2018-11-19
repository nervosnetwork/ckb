use bigint::H256;
use nervos_chain::chain::ChainProvider;
use nervos_protocol;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct GetDataProcess<'a, C: 'a> {
    message: &'a nervos_protocol::GetData,
    synchronizer: &'a Synchronizer<C>,
    nc: &'a NetworkContext,
}

impl<'a, C> GetDataProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a nervos_protocol::GetData,
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
            InventoryProcess::new(self.nc, self.synchronizer, inventory).execute();
        }
    }
}

pub struct InventoryProcess<'a, C: 'a> {
    nc: &'a NetworkContext,
    synchronizer: &'a Synchronizer<C>,
    inventory: &'a nervos_protocol::Inventory,
}

impl<'a, C> InventoryProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        nc: &'a NetworkContext,
        synchronizer: &'a Synchronizer<C>,
        inventory: &'a nervos_protocol::Inventory,
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
            nervos_protocol::InventoryType::MSG_BLOCK => {
                if let Some(ref block) = self
                    .synchronizer
                    .get_block(&H256::from(self.inventory.get_hash()))
                {
                    let mut payload = nervos_protocol::Payload::new();
                    payload.set_block(block.into());
                    let _ = self.nc.respond(payload);
                } else {
                    //Reponse notfound
                }
            }
            nervos_protocol::InventoryType::ERROR => {}
            _ => {}
        }
    }
}

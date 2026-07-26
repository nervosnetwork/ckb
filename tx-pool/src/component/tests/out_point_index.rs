use super::OutPointIndex;
use ckb_logger::debug;
use ckb_types::{
    bytes::Bytes,
    core::{error::OutPointError, tx_pool::Reject},
    packed::{OutPoint, ProposalShortId},
    prelude::Entity,
};
use std::collections::hash_map::Entry;

impl OutPointIndex {
    pub(crate) fn insert_input(
        &mut self,
        out_point: OutPoint,
        txid: ProposalShortId,
    ) -> Result<(), Reject> {
        let out_point = crate::util::compact_packed(&out_point);
        match self.inputs.entry(out_point.clone()) {
            Entry::Occupied(occupied) => {
                debug!(
                    "txpool unexpected double-spending out_point: {:?} old_tx: {:?} new_tx: {:?}",
                    out_point,
                    occupied.get(),
                    txid
                );
                Err(Reject::Resolve(OutPointError::Dead(out_point)))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(txid);
                Ok(())
            }
        }
    }

    pub(crate) fn inputs_len(&self) -> usize {
        self.inputs.len()
    }

    pub(crate) fn header_deps_len(&self) -> usize {
        self.header_deps.len()
    }

    pub(crate) fn deps_len(&self) -> usize {
        self.deps.len()
    }
}

fn shared_out_point() -> (OutPoint, *const u8) {
    let template = OutPoint::default();
    let len = template.as_slice().len();
    let mut raw = vec![0x33; 4_096];
    raw[2_048..2_048 + len].copy_from_slice(template.as_slice());
    let backing = Bytes::from(raw);
    let out_point = OutPoint::new_unchecked(backing.slice(2_048..2_048 + len));
    let ptr = out_point.as_slice().as_ptr();
    (out_point, ptr)
}

#[test]
fn persistent_outpoint_indexes_detach_shared_backing() {
    let mut index = OutPointIndex::default();
    let (input, input_ptr) = shared_out_point();
    index
        .insert_input(input, Default::default())
        .expect("vacant input");
    assert_ne!(
        index.inputs.keys().next().unwrap().as_slice().as_ptr(),
        input_ptr
    );

    let (dep, dep_ptr) = shared_out_point();
    index.insert_deps(dep, Default::default());
    assert_ne!(
        index.deps.keys().next().unwrap().as_slice().as_ptr(),
        dep_ptr
    );
}

//! Merkle Mountain Range
//!
//! references:
//! https://github.com/mimblewimble/grin/blob/master/doc/mmr.md#structure
//! https://github.com/mimblewimble/grin/blob/0ff6763ee64e5a14e70ddd4642b99789a1648a32/core/src/core/pmmr.rs#L606

use crate::helper::{get_peaks, parent_offset, pos_height_in_tree, sibling_offset};
use crate::mmr_store::{MMRBatch, MMRStore};
use crate::MerkleElem;
use crate::Result;
use std::borrow::Cow;
use std::convert::TryInto;
use std::marker::PhantomData;

#[derive(Clone)]
pub struct MMR<Elem, Store> {
    mmr_size: u64,
    store: Store,
    merkle_elem: PhantomData<Elem>,
}

impl<Elem: MerkleElem + Clone + Eq + Debug, Store: MMRStore<Elem>> MMR<Elem, Store> {
    pub fn new(mmr_size: u64, store: Store) -> Self {
        MMR {
            mmr_size,
            store,
            merkle_elem: PhantomData,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    // get data from memory hashes
    fn get_mem_data<'a>(
        &self,
        pos: u64,
        batch: &'a MMRBatch<Elem>,
        hashes: &'a [Elem],
    ) -> Result<Cow<'a, Elem>> {
        let pos_offset = pos.checked_sub(self.mmr_size);
        if let Some(elem) = pos_offset.and_then(|i| hashes.get(i as usize)) {
            return Ok(Cow::Borrowed(elem));
        }
        if let Some(elem) = batch.get_elem(pos)? {
            return Ok(Cow::Borrowed(elem));
        }
        Ok(Cow::Owned(self.store.get_elem(pos)?.expect("must exists")))
    }
    // push a element and return position
    pub fn push(&mut self, batch: &mut MMRBatch<Elem>, elem: Elem) -> Result<u64> {
        let mut elems: Vec<Elem> = Vec::new();
        // position of new elem
        let elem_pos = self.mmr_size;
        elems.push(elem);
        let mut height = 0u32;
        let mut pos = elem_pos;
        // continue to merge tree node if next pos heigher than current
        while pos_height_in_tree(pos + 1) > u64::from(height) {
            pos += 1;
            let left_pos = pos - parent_offset(height);
            let right_pos = left_pos + sibling_offset(height.try_into()?);
            let left_elem = self.get_mem_data(left_pos, &batch, &elems)?;
            let right_elem = self.get_mem_data(right_pos, &batch, &elems)?;
            let parent_elem = Elem::merge(&left_elem, &right_elem)?;
            elems.push(parent_elem);
            height += 1
        }
        // store hashes
        batch.append(elem_pos, elems)?;
        // update mmr_size
        self.mmr_size = pos + 1;
        Ok(elem_pos)
    }

    /// get_root
    pub fn get_root(&self, batch: Option<&MMRBatch<Elem>>) -> Result<Option<Elem>> {
        if self.mmr_size == 0 {
            return Ok(None);
        } else if self.mmr_size == 1 {
            if let Some(Ok(Some(elem))) = batch.map(|b| b.get_elem(0)) {
                return Ok(Some(elem.to_owned()));
            }
            return self.store.get_elem(0);
        }
        let peaks = get_peaks(self.mmr_size);
        self.bag_rhs_peaks(batch, 0, &peaks)
    }

    fn bag_rhs_peaks(
        &self,
        batch: Option<&MMRBatch<Elem>>,
        skip_peak_pos: u64,
        peaks: &[u64],
    ) -> Result<Option<Elem>> {
        let mut rhs_peak_elems: Vec<Elem> = peaks
            .iter()
            .filter(|&&p| p > skip_peak_pos)
            .map(|&p| {
                match batch {
                    Some(b) => match b.get_elem(p)? {
                        Some(v) => Ok(Some(v.clone())),
                        None => self.store.get_elem(p),
                    },
                    None => self.store.get_elem(p),
                }
                // batch.and_then(|b| b.get_elem(p).map(|o|o.map(Clone::clone))).or_else(||self.store.get_elem(p))
            })
            .collect::<Result<Option<_>>>()?
            .expect("data must exists");
        while rhs_peak_elems.len() > 1 {
            let right_peak = rhs_peak_elems.pop().expect("pop");
            let left_peak = rhs_peak_elems.pop().expect("pop");
            rhs_peak_elems.push(Elem::merge(&right_peak, &left_peak)?);
        }
        Ok(rhs_peak_elems.pop())
    }

    pub fn gen_proof(&self, mut pos: u64) -> Result<MerkleProof<Elem>> {
        let mut proof: Vec<Elem> = Vec::new();
        let mut height = 0;
        while pos < self.mmr_size {
            let pos_height = pos_height_in_tree(pos);
            let next_height = pos_height_in_tree(pos + 1);
            if next_height > pos_height {
                let sib_pos = pos - sibling_offset(height);
                if sib_pos > self.mmr_size - 1 {
                    break;
                }
                proof.push(self.store.get_elem(sib_pos)?.expect("must exists"));
                // go to next pos
                pos += 1;
            } else {
                let sib_pos = pos + sibling_offset(height);
                if sib_pos > self.mmr_size - 1 {
                    break;
                }
                proof.push(self.store.get_elem(sib_pos)?.expect("must exists"));
                pos += parent_offset(height);
            }
            height += 1;
        }
        // now we get peak merkle proof
        let peak_pos = pos;
        // calculate bagging proof
        let peaks = get_peaks(self.mmr_size);
        if let Some(rhs_peak_hash) = self.bag_rhs_peaks(None, peak_pos, &peaks[..])? {
            proof.push(rhs_peak_hash);
        }
        let lhs_peaks: Vec<_> = peaks
            .iter()
            .filter(|&&p| p < peak_pos)
            .map(|&p| self.store.get_elem(p))
            .rev()
            .collect::<Result<Option<_>>>()?
            .expect("must exists");
        proof.extend(lhs_peaks);
        Ok(MerkleProof::new(self.mmr_size, proof))
    }
}

#[derive(Debug)]
pub struct MerkleProof<Elem> {
    mmr_size: u64,
    proof: Vec<Elem>,
}

use std::fmt::Debug;

impl<Elem: MerkleElem + Eq + Debug> MerkleProof<Elem> {
    pub fn new(mmr_size: u64, proof: Vec<Elem>) -> Self {
        MerkleProof { mmr_size, proof }
    }

    pub fn verify(&self, root: Elem, mut pos: u64, elem: Elem) -> Result<bool> {
        let peaks = get_peaks(self.mmr_size);
        let mut sum_elem = elem;
        let mut height = 0;
        for proof in &self.proof {
            if peaks.contains(&pos) {
                sum_elem = if Some(&pos) == peaks.last() {
                    Elem::merge(&sum_elem, &proof)?
                } else {
                    pos = *peaks.last().expect("must exists");
                    Elem::merge(proof, &sum_elem)?
                };
                continue;
            }

            // verify merkle path
            let pos_height = pos_height_in_tree(pos);
            let next_height = pos_height_in_tree(pos + 1);
            sum_elem = if next_height > pos_height {
                // to next pos
                pos += 1;
                Elem::merge(proof, &sum_elem)?
            } else {
                pos += parent_offset(height);
                Elem::merge(&sum_elem, proof)?
            };
            height += 1
        }
        Ok(root == sum_elem)
    }
}

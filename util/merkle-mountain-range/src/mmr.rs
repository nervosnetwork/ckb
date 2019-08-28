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
use std::fmt::Debug;
use std::marker::PhantomData;

pub struct MMR<'a, Elem: MerkleElem, Store: MMRStore<Elem>> {
    mmr_size: u64,
    batch: &'a mut MMRBatch<Elem, Store>,
    merkle_elem: PhantomData<Elem>,
}

impl<'a, Elem: MerkleElem + Clone + PartialEq + Debug, Store: MMRStore<Elem>> MMR<'a, Elem, Store> {
    pub fn new(mmr_size: u64, batch: &'a mut MMRBatch<Elem, Store>) -> Self {
        MMR {
            mmr_size,
            batch,
            merkle_elem: PhantomData,
        }
    }

    // get data from memory hashes
    fn find_elem<'b>(&self, pos: u64, hashes: &'b [Elem]) -> Result<Cow<'b, Elem>> {
        let pos_offset = pos.checked_sub(self.mmr_size);
        if let Some(elem) = pos_offset.and_then(|i| hashes.get(i as usize)) {
            return Ok(Cow::Borrowed(elem));
        }
        Ok(Cow::Owned(self.batch.get_elem(pos)?.expect("must exists")))
    }

    pub fn mmr_size(&self) -> u64 {
        self.mmr_size
    }
    // push a element and return position
    pub fn push(&mut self, elem: Elem) -> Result<u64> {
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
            let left_elem = self.find_elem(left_pos, &elems)?;
            let right_elem = self.find_elem(right_pos, &elems)?;
            let parent_elem = Elem::merge(&left_elem, &right_elem)?;
            elems.push(parent_elem);
            height += 1
        }
        // store hashes
        self.batch.append(elem_pos, elems)?;
        // update mmr_size
        self.mmr_size = pos + 1;
        Ok(elem_pos)
    }

    /// get_root
    pub fn get_root(&self) -> Result<Option<Elem>> {
        if self.mmr_size == 0 {
            return Ok(None);
        } else if self.mmr_size == 1 {
            return self.batch.get_elem(0);
        }
        let peaks = get_peaks(self.mmr_size);
        self.bag_rhs_peaks(0, &peaks)
    }

    fn bag_rhs_peaks(&self, skip_peak_pos: u64, peaks: &[u64]) -> Result<Option<Elem>> {
        let mut rhs_peak_elems: Vec<Elem> = peaks
            .iter()
            .filter(|&&p| p > skip_peak_pos)
            .map(|&p| self.batch.get_elem(p))
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
                // implies pos is right sibling
                let sib_pos = pos - sibling_offset(height);
                if sib_pos > self.mmr_size - 1 {
                    break;
                }
                proof.push(self.batch.get_elem(sib_pos)?.expect("must exists"));
                // go to next pos
                pos += 1;
            } else {
                // pos is left sibling
                let sib_pos = pos + sibling_offset(height);
                if sib_pos > self.mmr_size - 1 {
                    break;
                }
                proof.push(self.batch.get_elem(sib_pos)?.expect("must exists"));
                pos += parent_offset(height);
            }
            height += 1;
        }
        // now we get peak merkle proof
        let peak_pos = pos;
        // calculate bagging proof
        let peaks = get_peaks(self.mmr_size);
        if let Some(rhs_peak_hash) = self.bag_rhs_peaks(peak_pos, &peaks[..])? {
            proof.push(rhs_peak_hash);
        }
        let lhs_peaks: Vec<_> = peaks
            .iter()
            .filter(|&&p| p < peak_pos)
            .map(|&p| self.batch.get_elem(p))
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

impl<Elem: MerkleElem + PartialEq + Debug> MerkleProof<Elem> {
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

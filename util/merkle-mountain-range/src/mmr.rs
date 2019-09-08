//! Merkle Mountain Range
//!
//! references:
//! https://github.com/mimblewimble/grin/blob/master/doc/mmr.md#structure
//! https://github.com/mimblewimble/grin/blob/0ff6763ee64e5a14e70ddd4642b99789a1648a32/core/src/core/pmmr.rs#L606

use crate::helper::{get_peaks, parent_offset, pos_height_in_tree, sibling_offset};
use crate::mmr_store::{MMRBatch, MMRStore};
use crate::{Error, Merge, Result};
use std::borrow::Cow;
use std::fmt::Debug;
use std::marker::PhantomData;

pub struct MMR<T, M, S: MMRStore<T>> {
    mmr_size: u64,
    batch: MMRBatch<T, S>,
    merge: PhantomData<M>,
}

impl<'a, T: Clone + PartialEq + Debug, M: Merge<Item = T>, S: MMRStore<T>> MMR<T, M, S> {
    pub fn new(mmr_size: u64, store: S) -> Self {
        MMR {
            mmr_size,
            batch: MMRBatch::new(store),
            merge: PhantomData,
        }
    }

    // find internal MMR elem, the pos must exists, otherwise a error will return
    fn find_elem<'b>(&self, pos: u64, hashes: &'b [T]) -> Result<Cow<'b, T>> {
        let pos_offset = pos.checked_sub(self.mmr_size);
        if let Some(elem) = pos_offset.and_then(|i| hashes.get(i as usize)) {
            return Ok(Cow::Borrowed(elem));
        }
        let elem = self.batch.get_elem(pos)?.ok_or(Error::InconsistentStore)?;
        Ok(Cow::Owned(elem))
    }

    pub fn mmr_size(&self) -> u64 {
        self.mmr_size
    }

    pub fn is_empty(&self) -> bool {
        self.mmr_size == 0
    }

    // push a element and return position
    pub fn push(&mut self, elem: T) -> Result<u64> {
        let mut elems: Vec<T> = Vec::new();
        // position of new elem
        let elem_pos = self.mmr_size;
        elems.push(elem);
        let mut height = 0u32;
        let mut pos = elem_pos;
        // continue to merge tree node if next pos heigher than current
        while pos_height_in_tree(pos + 1) > height {
            pos += 1;
            let left_pos = pos - parent_offset(height);
            let right_pos = left_pos + sibling_offset(height);
            let left_elem = self.find_elem(left_pos, &elems)?;
            let right_elem = self.find_elem(right_pos, &elems)?;
            let parent_elem = M::merge(&left_elem, &right_elem);
            elems.push(parent_elem);
            height += 1
        }
        // store hashes
        self.batch.append(elem_pos, elems);
        // update mmr_size
        self.mmr_size = pos + 1;
        Ok(elem_pos)
    }

    /// get_root
    pub fn get_root(&self) -> Result<T> {
        if self.mmr_size == 0 {
            return Err(Error::GetRootOnEmpty);
        } else if self.mmr_size == 1 {
            return self.batch.get_elem(0)?.ok_or(Error::InconsistentStore);
        }
        let peaks = get_peaks(self.mmr_size);
        self.bag_rhs_peaks(0, &peaks)?
            .ok_or(Error::InconsistentStore)
    }

    fn bag_rhs_peaks(&self, skip_peak_pos: u64, peaks: &[u64]) -> Result<Option<T>> {
        let mut rhs_peak_elems: Vec<T> = peaks
            .iter()
            .filter(|&&p| p > skip_peak_pos)
            .map(|&p| self.batch.get_elem(p))
            .collect::<Result<Option<_>>>()?
            .ok_or(Error::InconsistentStore)?;
        while rhs_peak_elems.len() > 1 {
            let right_peak = rhs_peak_elems.pop().expect("pop");
            let left_peak = rhs_peak_elems.pop().expect("pop");
            rhs_peak_elems.push(M::merge(&right_peak, &left_peak));
        }
        Ok(rhs_peak_elems.pop())
    }

    pub fn gen_proof(&self, mut pos: u64) -> Result<MerkleProof<T, M>> {
        let mut proof: Vec<T> = Vec::new();
        let mut height = 0;
        while pos < self.mmr_size {
            let pos_height = pos_height_in_tree(pos);
            let next_height = pos_height_in_tree(pos + 1);
            let (sib_pos, next_pos) = if next_height > pos_height {
                // implies pos is right sibling
                let sib_pos = pos - sibling_offset(height);
                (sib_pos, pos + 1)
            } else {
                // pos is left sibling
                let sib_pos = pos + sibling_offset(height);
                (sib_pos, pos + parent_offset(height))
            };
            if sib_pos > self.mmr_size - 1 {
                break;
            }
            proof.push(
                self.batch
                    .get_elem(sib_pos)?
                    .ok_or(Error::InconsistentStore)?,
            );
            pos = next_pos;
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
            .ok_or(Error::InconsistentStore)?;
        proof.extend(lhs_peaks);
        Ok(MerkleProof::new(self.mmr_size, proof))
    }

    pub fn commit(self) -> Result<()> {
        self.batch.commit()
    }
}

#[derive(Debug)]
pub struct MerkleProof<T, M> {
    mmr_size: u64,
    proof: Vec<T>,
    merge: PhantomData<M>,
}

impl<T: PartialEq + Debug, M: Merge<Item = T>> MerkleProof<T, M> {
    pub fn new(mmr_size: u64, proof: Vec<T>) -> Self {
        MerkleProof {
            mmr_size,
            proof,
            merge: PhantomData,
        }
    }

    pub fn verify(&self, root: T, mut pos: u64, elem: T) -> Result<bool> {
        let peaks = get_peaks(self.mmr_size);
        let mut sum_elem = elem;
        let mut height = 0;
        let mut proof_iter = self.proof.iter();
        // calculate peak's merkle root
        // start bagging peaks if pos reach a peak pos
        while !peaks.contains(&pos) {
            let proof = match proof_iter.next() {
                Some(proof) => proof,
                None => break,
            };
            // verify merkle path
            let pos_height = pos_height_in_tree(pos);
            let next_height = pos_height_in_tree(pos + 1);
            sum_elem = if next_height > pos_height {
                // to next pos
                pos += 1;
                M::merge(proof, &sum_elem)
            } else {
                pos += parent_offset(height);
                M::merge(&sum_elem, proof)
            };
            height += 1
        }

        // bagging peaks
        // bagging with left peaks if pos is last peak
        let mut bagging_left = Some(&pos) == peaks.last();
        for proof in &mut proof_iter {
            sum_elem = if bagging_left {
                M::merge(&sum_elem, &proof)
            } else {
                // we are not in the last peak, so bag with right peaks first
                // notice the right peaks is already bagging into one hash in proof,
                // so after this merge, the remain proofs are always left peaks.
                bagging_left = true;
                M::merge(&proof, &sum_elem)
            };
        }
        Ok(root == sum_elem)
    }
}

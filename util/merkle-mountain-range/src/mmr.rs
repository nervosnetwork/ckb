//! Merkle Mountain Range
//!
//! references:
//! https://github.com/mimblewimble/grin/blob/master/doc/mmr.md#structure
//! https://github.com/mimblewimble/grin/blob/0ff6763ee64e5a14e70ddd4642b99789a1648a32/core/src/core/pmmr.rs#L606

use crate::error::Result;
use crate::mmr_store::MMRStore;
use crate::MerkleElem;
use ckb_db::KeyValueDB;
use std::borrow::Cow;
use std::convert::TryInto;
use std::marker::PhantomData;
use std::sync::Arc;

pub struct MMR<Elem, DB> {
    mmr_size: u64,
    store: Arc<MMRStore<Elem, DB>>,
    merkle_elem: PhantomData<Elem>,
}

fn pos_height_in_tree(mut pos: u64) -> u64 {
    pos += 1;
    fn all_ones(num: u64) -> bool {
        num.count_zeros() == num.leading_zeros()
    }
    fn jump_left(pos: u64) -> u64 {
        let bit_length = 64 - pos.leading_zeros();
        let most_significant_bits = 1 << (bit_length - 1);
        pos - (most_significant_bits - 1)
    }

    while !all_ones(pos) {
        pos = jump_left(pos)
    }

    (64 - pos.leading_zeros() - 1).into()
}

fn parent_offset(height: u32) -> u64 {
    2 << height
}

fn sibling_offset(height: u32) -> u64 {
    (2 << height) - 1
}

fn get_peaks(mmr_size: u64) -> Vec<u64> {
    let mut pos_s = Vec::new();
    let (mut height, mut pos) = left_peak_height_pos(mmr_size);
    pos_s.push(pos);
    while height > 0 {
        let peak = match get_right_peak(height, pos, mmr_size) {
            Some(peak) => peak,
            None => break,
        };
        height = peak.0;
        pos = peak.1;
        pos_s.push(pos);
    }
    pos_s
}

fn get_right_peak(mut height: u32, mut pos: u64, mmr_size: u64) -> Option<(u32, u64)> {
    // move to right sibling pos
    pos += sibling_offset(height);
    // loop until we find a pos in mmr
    while pos > mmr_size - 1 {
        if height == 0 {
            return None;
        }
        // move to left child
        pos -= parent_offset(height - 1);
        height -= 1;
    }
    Some((height, pos))
}

fn left_peak_height_pos(mmr_size: u64) -> (u32, u64) {
    fn get_left_pos(height: u32) -> u64 {
        (1 << (height + 1)) - 2
    }
    let mut height = 0;
    let mut prev_pos = 0;
    let mut pos = get_left_pos(height);
    while pos < mmr_size {
        height += 1;
        prev_pos = pos;
        pos = get_left_pos(height);
    }
    (height - 1, prev_pos)
}

impl<Elem: MerkleElem + Clone + Eq + Debug, DB: KeyValueDB> MMR<Elem, DB> {
    pub fn new(mmr_size: u64, store: Arc<MMRStore<Elem, DB>>) -> Self {
        MMR {
            store,
            mmr_size,
            merkle_elem: PhantomData,
        }
    }

    // get data from memory hashes
    fn get_mem_data<'a>(&self, pos: u64, hashes: &'a [Elem]) -> Result<Cow<'a, Elem>> {
        let pos_offset = pos.checked_sub(self.mmr_size);
        match pos_offset.and_then(|i| hashes.get(i as usize)) {
            Some(elem) => Ok(Cow::Borrowed(elem)),
            None => Ok(Cow::Owned(self.store.get_elem(pos)?.expect("must exists"))),
        }
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
            let left_elem = self.get_mem_data(left_pos, &elems)?;
            let right_elem = self.get_mem_data(right_pos, &elems)?;
            elems.push(Elem::merge(&left_elem, &right_elem)?);
            height += 1
        }
        // store hashes
        self.store.append(elem_pos, &elems)?;
        // update mmr_size
        self.mmr_size = pos + 1;
        Ok(elem_pos)
    }

    /// get_root
    pub fn get_root(&self) -> Result<Option<Elem>> {
        if self.mmr_size == 1 {
            return self.store.get_elem(0);
        }
        let peaks = get_peaks(self.mmr_size);
        self.bag_rhs_peaks(0, &peaks)
    }

    fn bag_rhs_peaks(&self, skip_peak_pos: u64, peaks: &[u64]) -> Result<Option<Elem>> {
        let mut rhs_peak_elems: Vec<Elem> = peaks
            .iter()
            .filter(|&&p| p > skip_peak_pos)
            .map(|&p| self.store.get_elem(p))
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
        if let Some(rhs_peak_hash) = self.bag_rhs_peaks(peak_pos, &peaks[..])? {
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

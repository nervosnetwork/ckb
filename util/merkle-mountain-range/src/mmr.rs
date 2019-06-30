//! Merkle Mountain Range
//!
//! references:
//! https://github.com/mimblewimble/grin/blob/master/doc/mmr.md#structure
//! https://github.com/mimblewimble/grin/blob/0ff6763ee64e5a14e70ddd4642b99789a1648a32/core/src/core/pmmr.rs#L606

use bytes::Bytes;
use ckb_db::{Col, DbBatch, KeyValueDB, Result as DbResult};
use ckb_hash::Blake2bWriter;
use failure::Error;
use std::convert::TryInto;
use std::io::Write;

pub struct MMR<DB> {
    mmr_size: u64,
    store: MMRStore<DB>,
}

pub type Result<T> = ::std::result::Result<T, Error>;

pub trait Hashable {
    fn hash<W: Write>(self, hasher: &mut W) -> Result<()>;
}

impl Hashable for &Bytes {
    fn hash<W: Write>(self, hasher: &mut W) -> Result<()> {
        hasher.write_all(self)?;
        Ok(())
    }
}

fn merge_hash<H: Hashable>(lhs: H, rhs: H) -> Result<Bytes> {
    let mut hasher = Blake2bWriter::new();
    lhs.hash(&mut hasher)?;
    rhs.hash(&mut hasher)?;
    Ok(Bytes::from(&hasher.finalize()[..]))
}

fn get_hash<H: Hashable>(elem: H) -> Result<Bytes> {
    let mut hasher = Blake2bWriter::new();
    elem.hash(&mut hasher)?;
    Ok(Bytes::from(&hasher.finalize()[..]))
}

fn tree_height(mut pos: u64) -> u64 {
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

fn sibling_offset(height: u32) -> u64 {
    return (2 << height) - 1;
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
        pos -= 2 << height - 1;
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

pub struct MMRStore<DB: Sized> {
    db: DB,
    col: Col,
}

impl<DB: KeyValueDB> MMRStore<DB> {
    pub fn new(db: DB, col: Col) -> Self {
        MMRStore { db, col }
    }
    fn get_data(&self, pos: u64) -> DbResult<Option<Bytes>> {
        self.db
            .read(self.col, &pos.to_le_bytes()[..])
            .map(|r| r.map(Into::into))
    }
    fn append(&self, pos: u64, hashes: &[Bytes]) -> DbResult<()> {
        let mut batch = self.db.batch()?;
        for (offset, hash) in hashes.into_iter().enumerate() {
            let pos: u64 = pos + (offset as u64);
            batch.insert(self.col, &pos.to_le_bytes()[..], hash)?;
        }
        batch.commit()
    }
}

impl<DB: KeyValueDB> MMR<DB> {
    pub fn new(mmr_size: u64, store: MMRStore<DB>) -> Self {
        MMR { store, mmr_size }
    }

    // get data from memory hashes
    fn get_mem_data(&self, pos: u64, hashes: &[Bytes]) -> DbResult<Bytes> {
        let pos_offset = pos.checked_sub(self.mmr_size);
        match pos_offset.and_then(|i| hashes.get(i as usize)) {
            Some(hash) => Ok(hash.to_owned()),
            None => Ok(self.store.get_data(pos)?.expect("must exists")),
        }
    }
    // push a element and return position
    pub fn push<T: Hashable>(&mut self, elem: T) -> Result<u64> {
        let mut hashes = Vec::new();
        // position of new elem
        let elem_pos = self.mmr_size;
        hashes.push(get_hash(elem)?);
        let mut height = 0;
        let mut pos = elem_pos;
        // continue to merge tree node if next pos heigher than current
        while tree_height(pos + 1) > height {
            pos += 1;
            let left_pos = pos - (2 << height);
            let right_pos = left_pos + sibling_offset(height.try_into()?);
            let left_elem = self.get_mem_data(left_pos, &hashes)?;
            let right_elem = self.get_mem_data(right_pos, &hashes)?;
            hashes.push(merge_hash(&left_elem, &right_elem)?);
            height += 1
        }
        // store hashes
        self.store.append(elem_pos, &hashes)?;
        // update mmr_size
        self.mmr_size = pos + 1;
        Ok(elem_pos)
    }

    /// get_root
    pub fn get_root(&self) -> Result<Option<Bytes>> {
        let peaks = get_peaks(self.mmr_size);
        self.bag_rhs_peaks(0, &peaks)
    }

    fn bag_rhs_peaks(&self, skip_peak_pos: u64, peaks: &[u64]) -> Result<Option<Bytes>> {
        let mut rhs_peak_hashes: Vec<Bytes> = peaks
            .into_iter()
            .filter(|&&p| p > skip_peak_pos)
            .map(|&p| self.store.get_data(p))
            .collect::<DbResult<Option<_>>>()?
            .expect("data must exists");
        while rhs_peak_hashes.len() > 1 {
            let right_peak = rhs_peak_hashes.pop().expect("pop");
            let left_peak = rhs_peak_hashes.pop().expect("pop");
            rhs_peak_hashes.push(merge_hash(&right_peak, &left_peak)?);
        }
        Ok(rhs_peak_hashes.pop())
    }

    pub fn gen_proof(&self, mut pos: u64) -> Result<MerkleProof> {
        let mut proof = Vec::new();
        let mut height = 0;
        while pos < self.mmr_size {
            let pos_height = tree_height(pos);
            let next_height = tree_height(pos + 1);
            if next_height > pos_height {
                let sib_pos = pos - sibling_offset(height);
                if sib_pos > self.mmr_size - 1 {
                    break;
                }
                proof.push(self.store.get_data(sib_pos)?.expect("must exists"));
                pos += 1;
            } else {
                let sib_pos = pos + sibling_offset(height);
                if sib_pos > self.mmr_size - 1 {
                    break;
                }
                proof.push(self.store.get_data(sib_pos)?.expect("must exists"));
                pos += 2 << height;
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
            .map(|&p| self.store.get_data(p))
            .rev()
            .collect::<DbResult<Option<_>>>()?
            .expect("must exists");
        proof.extend(lhs_peaks);
        Ok(MerkleProof::new(self.mmr_size, proof))
    }
}

pub struct MerkleProof {
    mmr_size: u64,
    proof: Vec<Bytes>,
}

impl MerkleProof {
    pub fn new(mmr_size: u64, proof: Vec<Bytes>) -> Self {
        MerkleProof { mmr_size, proof }
    }

    pub fn verify<H: Hashable>(&self, root: Bytes, mut pos: u64, elem: H) -> Result<bool> {
        let peaks = get_peaks(self.mmr_size);
        let mut elem_hash = get_hash(elem)?;
        let mut height = 0;
        for proof in &self.proof {
            if peaks.contains(&pos) {
                elem_hash = if Some(&pos) == peaks.last() {
                    merge_hash(&elem_hash, &proof)?
                } else {
                    pos = *peaks.last().expect("must exists");
                    merge_hash(proof, &elem_hash)?
                };
                continue;
            }

            // verify merkle path
            let pos_height = tree_height(pos);
            let next_height = tree_height(pos + 1);
            elem_hash = if next_height > pos_height {
                pos += 1;
                merge_hash(proof, &elem_hash)?
            } else {
                pos += 2 << height;
                merge_hash(&elem_hash, proof)?
            };
            height += 1
        }
        Ok(root == elem_hash)
    }
}

use crate::MerkleElem;
use crate::Result;

#[derive(Default)]
pub struct MMRBatch<Elem: MerkleElem>(Vec<(u64, Vec<Elem>)>);

impl<Elem: MerkleElem> MMRBatch<Elem> {
    pub fn new() -> Self {
        MMRBatch(Vec::new())
    }
    pub fn append(&mut self, pos: u64, elems: Vec<Elem>) -> Result<()> {
        self.0.push((pos, elems));
        Ok(())
    }
    pub fn get_elem(&self, pos: u64) -> Result<Option<&Elem>> {
        for (start_pos, elems) in self.0.iter().rev() {
            if pos < *start_pos {
                continue;
            } else if pos < start_pos + elems.len() as u64 {
                return Ok(elems.get((pos - start_pos) as usize));
            } else {
                break;
            }
        }
        Ok(None)
    }
}

impl<Elem: MerkleElem> IntoIterator for MMRBatch<Elem> {
    type Item = (u64, Vec<Elem>);
    type IntoIter = ::std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

pub trait MMRStore<Elem: MerkleElem> {
    fn get_elem(&self, pos: u64) -> Result<Option<Elem>>;
}

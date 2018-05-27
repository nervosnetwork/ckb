use bigint::H256;
use core::header::Header;
use std::collections::{HashMap, HashSet, VecDeque};

/// Block position
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Position {
    None,
    Front,
    Index(usize),
}

#[derive(Debug, Clone, Default)]
pub struct HashQueue {
    queue: VecDeque<H256>,
    set: HashSet<H256>,
}

#[allow(dead_code)]
impl HashQueue {
    pub fn new() -> Self {
        HashQueue {
            queue: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    /// Clears the queue
    pub fn clear(&mut self) {
        self.set.clear();
        self.queue.clear();
    }

    /// Returns len of the given queue.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns front element from the given queue.
    pub fn front(&self) -> Option<H256> {
        self.queue.front().cloned()
    }

    /// Returns back element from the given queue.
    pub fn back(&self) -> Option<H256> {
        self.queue.back().cloned()
    }

    /// Returns position of the element in the queue
    pub fn position(&self, hash: &H256) -> Option<usize> {
        self.queue.iter().position(|h| hash == h)
    }

    /// Returns element at position
    pub fn at(&self, position: usize) -> Option<H256> {
        self.queue.get(position).cloned()
    }

    /// Returns previous-to back element from the given queue.
    pub fn pre_back(&self) -> Option<H256> {
        let queue_len = self.queue.len();
        if queue_len <= 1 {
            return None;
        }
        Some(self.queue[queue_len - 2])
    }

    /// Returns true if queue contains element.
    pub fn contains(&self, hash: &H256) -> bool {
        self.set.contains(hash)
    }

    /// Returns n elements from the front of the queue
    pub fn front_n(&self, n: usize) -> Vec<H256> {
        self.queue.iter().take(n).cloned().collect()
    }

    /// Removes element from the front of the queue.
    pub fn pop_front(&mut self) -> Option<H256> {
        match self.queue.pop_front() {
            Some(hash) => {
                self.set.remove(&hash);
                Some(hash)
            }
            None => None,
        }
    }

    /// Removes n elements from the front of the queue.
    pub fn pop_front_n(&mut self, n: u32) -> Vec<H256> {
        let mut result: Vec<H256> = Vec::new();
        for _ in 0..n {
            match self.pop_front() {
                Some(hash) => result.push(hash),
                None => return result,
            }
        }
        result
    }

    /// Removes element from the back of the queue.
    pub fn pop_back(&mut self) -> Option<H256> {
        match self.queue.pop_back() {
            Some(hash) => {
                self.set.remove(&hash);
                Some(hash)
            }
            None => None,
        }
    }

    /// Adds element to the back of the queue.
    pub fn push_back(&mut self, hash: H256) {
        if !self.set.insert(hash) {
            panic!("must be checked by caller");
        }
        self.queue.push_back(hash);
    }

    /// Adds elements to the back of the queue.
    pub fn push_back_n(&mut self, hashes: Vec<H256>) {
        for hash in hashes {
            self.push_back(hash);
        }
    }

    /// Removes element from the queue, returning its position.
    pub fn remove(&mut self, hash: &H256) -> Position {
        if !self.set.remove(hash) {
            return Position::None;
        }

        if self.queue.front().expect("checked one line above") == hash {
            self.queue.pop_front();
            return Position::Front;
        }

        if let Some(position) = self.queue.iter().position(|h| h == hash) {
            self.queue.remove(position);
            return Position::Index(position);
        }

        // unreachable because hash is not missing, not at the front and not inside
        unreachable!()
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum BlockQueueState {
    Scheduled,
    Requested,
    Verifying,
}

#[derive(Debug, Default)]
pub struct BlockQueue {
    pub scheduled: HashQueue,
    pub requested: HashQueue,
    pub verifying: HashQueue,
}

#[allow(dead_code)]
impl BlockQueue {
    pub fn new() -> Self {
        BlockQueue {
            scheduled: HashQueue::new(),
            requested: HashQueue::new(),
            verifying: HashQueue::new(),
        }
    }

    pub fn at(&self, mut index: usize) -> Option<H256> {
        for queue in &[&self.scheduled, &self.requested, &self.verifying] {
            let queue_len = queue.len();
            if index < queue_len {
                return queue.at(index);
            }

            index -= queue_len;
        }
        None
    }

    pub fn len(&self) -> usize {
        self.scheduled.len() + self.requested.len() + self.verifying.len()
    }

    pub fn contains(&self, hash: &H256) -> Option<BlockQueueState> {
        if self.scheduled.contains(hash) {
            return Some(BlockQueueState::Scheduled);
        }
        if self.requested.contains(hash) {
            return Some(BlockQueueState::Requested);
        }
        if self.verifying.contains(hash) {
            return Some(BlockQueueState::Verifying);
        }
        None
    }
}

pub struct HeaderQueue {
    /// Best hash in storage
    pub storage_best_hash: H256,
    /// Headers by hash
    pub headers: HashMap<H256, Header>,
    /// Best chain
    pub best: HashQueue,
}

#[allow(dead_code)]
impl HeaderQueue {
    /// Create new best headers chain
    pub fn new(storage_best_hash: H256) -> Self {
        HeaderQueue {
            storage_best_hash,
            headers: HashMap::new(),
            best: HashQueue::new(),
        }
    }

    /// Get header from main chain at given position
    pub fn at(&self, height: u64) -> Option<Header> {
        self.best
            .at(height as usize)
            .and_then(|hash| self.headers.get(&hash).cloned())
    }

    /// Get geader by given hash
    pub fn by_hash(&self, hash: &H256) -> Option<Header> {
        self.headers.get(hash).cloned()
    }

    /// Get height of main chain
    pub fn height(&self, hash: &H256) -> Option<u64> {
        self.best.position(hash).map(|pos| pos as u64)
    }

    /// Get all direct child blocks hashes of given block hash
    pub fn children(&self, hash: &H256) -> Vec<H256> {
        self.best
            .position(hash)
            .and_then(|pos| self.best.at(pos + 1))
            .and_then(|child| Some(vec![child]))
            .unwrap_or_default()
    }

    /// Get hash of best block
    pub fn best_block_hash(&self) -> H256 {
        self.best
            .back()
            .or_else(|| Some(self.storage_best_hash))
            .expect("storage_best_hash is always known")
    }

    /// Insert new block header
    pub fn insert(&mut self, header: Header) {
        // append to the best chain
        if self.best_block_hash() == header.parent_hash {
            let header_hash = header.hash();
            self.headers.insert(header_hash, header);
            self.best.push_back(header_hash);
            return;
        }
    }

    /// Insert new blocks headers
    pub fn insert_n(&mut self, headers: Vec<Header>) {
        for header in headers {
            self.insert(header);
        }
    }

    /// Remove block header with given hash and all its children
    pub fn remove(&mut self, hash: &H256) {
        if self.headers.remove(hash).is_some() {
            match self.best.remove(hash) {
                Position::Front => self.clear(),
                Position::Index(position) => self.clear_after(position),
                _ => (),
            }
        }
    }

    /// Remove blocks headers with given hash and all its children
    pub fn remove_n(&mut self, hashes: impl IntoIterator<Item = H256>) {
        for hash in hashes {
            self.remove(&hash);
        }
    }

    /// Called when new blocks is inserted to storage
    pub fn block_inserted_to_storage(&mut self, hash: &H256) {
        if self.best.front().map(|h| &h == hash).unwrap_or(false) {
            self.best.pop_front();
            self.headers.remove(hash);
        }
        self.storage_best_hash = *hash;
    }

    /// Clears headers chain
    pub fn clear(&mut self) {
        self.headers.clear();
        self.best.clear();
    }

    /// Remove headers after position
    fn clear_after(&mut self, position: usize) {
        if position == 0 {
            self.clear()
        } else {
            while self.best.len() > position {
                self.headers
                    .remove(&self.best.pop_back().expect("len() > position; qed"));
            }
        }
    }
}

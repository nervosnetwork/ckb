use numext_fixed_hash::H256;

#[derive(Debug, Default)]
/// How to move current head to the specific head.
pub struct HeadRoute {
    /// The new head hash.
    pub head: H256,

    /// The hash of blocks that should be rolled back from newest to oldest.
    pub rollback: Vec<H256>,

    /// The hash of blocks that should be appended from oldest to newest.
    pub append: Vec<H256>,
}

impl HeadRoute {
    pub fn new(head: H256) -> HeadRoute {
        HeadRoute {
            head,
            rollback: Vec::new(),
            append: Vec::new(),
        }
    }
}

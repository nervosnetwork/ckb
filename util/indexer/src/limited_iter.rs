pub(crate) struct LimitedIterator<I> {
    inner: I,
    limit: usize,
    count: usize,
}

impl<I> LimitedIterator<I> {
    pub fn new(inner: I, limit: usize) -> Self {
        Self {
            inner,
            limit,
            count: 0,
        }
    }
}

impl<I: Iterator> Iterator for LimitedIterator<I> {
    type Item = Result<I::Item, &'static str>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.count >= self.limit {
            Some(Err("Iteration limit exceeded"))
        } else {
            self.count += 1;
            self.inner.next().map(Ok)
        }
    }
}

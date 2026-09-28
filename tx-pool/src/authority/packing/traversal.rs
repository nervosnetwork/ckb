//! Reusable DFS storage. Only a begun pass can mark a graph index as visited.

use super::PackingError;

pub(super) struct Traversal {
    marks: Vec<usize>,
    generation: usize,
    pub(super) stack: Vec<usize>,
}

pub(super) struct TraversalPass<'a> {
    marks: &'a mut [usize],
    generation: usize,
    pub(super) stack: &'a mut Vec<usize>,
}

impl Traversal {
    pub(super) fn new(len: usize) -> Self {
        Self {
            marks: vec![0; len],
            generation: 0,
            stack: Vec::new(),
        }
    }

    pub(super) fn begin(&mut self) -> Result<TraversalPass<'_>, PackingError> {
        self.stack.clear();
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(PackingError::Arithmetic)?;
        Ok(TraversalPass {
            marks: &mut self.marks,
            generation: self.generation,
            stack: &mut self.stack,
        })
    }
}

impl TraversalPass<'_> {
    /// Whether this is the first visit to a compiled graph index in this pass.
    #[expect(
        clippy::indexing_slicing,
        reason = "Traversal storage is sized to the compiled graph supplying these indices."
    )]
    pub(super) fn visit(&mut self, index: usize) -> bool {
        let mark = &mut self.marks[index];
        if *mark == self.generation {
            return false;
        }
        *mark = self.generation;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_are_unique_within_each_pass_and_scratch_is_reused() {
        let mut traversal = Traversal::new(3);
        for _ in 0..2 {
            let mut pass = traversal.begin().unwrap();
            assert!(pass.stack.is_empty());
            for index in [2, 0, 1] {
                assert!(pass.visit(index));
                assert!(!pass.visit(index));
            }
            pass.stack.push(2);
        }
    }

    #[test]
    fn exhausted_generation_cannot_reuse_old_marks() {
        let mut traversal = Traversal::new(1);
        traversal.generation = usize::MAX - 1;
        assert!(traversal.begin().unwrap().visit(0));
        assert!(matches!(traversal.begin(), Err(PackingError::Arithmetic)));
    }
}

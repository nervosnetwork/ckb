mod from_common;
mod from_storage;
mod to_common;
mod to_storage;

pub(crate) struct FlatbuffersVectorIterator<'a, T: flatbuffers::Follow<'a> + 'a> {
    vector: flatbuffers::Vector<'a, T>,
    counter: usize,
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> FlatbuffersVectorIterator<'a, T> {
    pub fn new(vector: flatbuffers::Vector<'a, T>) -> Self {
        Self { vector, counter: 0 }
    }
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> Iterator for FlatbuffersVectorIterator<'a, T> {
    type Item = T::Inner;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter < self.vector.len() {
            let result = self.vector.get(self.counter);
            self.counter += 1;
            Some(result)
        } else {
            None
        }
    }
}

pub(crate) trait FbVecIntoIterator<'a, T: flatbuffers::Follow<'a> + 'a> {
    fn iter(self) -> FlatbuffersVectorIterator<'a, T>;
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> FbVecIntoIterator<'a, T> for flatbuffers::Vector<'a, T> {
    fn iter(self) -> FlatbuffersVectorIterator<'a, T> {
        FlatbuffersVectorIterator::new(self)
    }
}

pub(crate) trait OptionShouldBeSome<T> {
    fn unwrap_some(self) -> crate::Result<T>;
}

impl<T> OptionShouldBeSome<T> for Option<T> {
    fn unwrap_some(self) -> crate::Result<T> {
        self.ok_or(crate::Error::Deserialize)
    }
}

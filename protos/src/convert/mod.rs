use flatbuffers::{FlatBufferBuilder, Follow, Vector, WIPOffset};

mod from_common;
mod from_storage;
mod to_common;
mod to_storage;

pub(crate) struct FlatbuffersVectorIterator<'a, T: Follow<'a> + 'a> {
    vector: Vector<'a, T>,
    counter: usize,
}

impl<'a, T: Follow<'a> + 'a> FlatbuffersVectorIterator<'a, T> {
    pub fn new(vector: Vector<'a, T>) -> Self {
        Self { vector, counter: 0 }
    }
}

impl<'a, T: Follow<'a> + 'a> Iterator for FlatbuffersVectorIterator<'a, T> {
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

pub(crate) trait FbVecIntoIterator<'a, T: Follow<'a> + 'a> {
    fn iter(self) -> FlatbuffersVectorIterator<'a, T>;
}

impl<'a, T: Follow<'a> + 'a> FbVecIntoIterator<'a, T> for Vector<'a, T> {
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

pub trait CanBuild<'a>: Sized {
    type Input: ?Sized;

    fn build<'b: 'a>(fbb: &mut FlatBufferBuilder<'b>, st: &Self::Input) -> WIPOffset<Self>;

    fn full_build(st: &Self::Input) -> DataBuilder {
        let mut builder = FlatBufferBuilder::new();
        let proto = Self::build(&mut builder, st);
        builder.finish(proto, None);
        DataBuilder(builder)
    }
}

pub struct DataBuilder<'a>(FlatBufferBuilder<'a>);

impl DataBuilder<'_> {
    pub fn as_slice(&self) -> &[u8] {
        self.0.finished_data()
    }
}

macro_rules! impl_conversion_for_entity_unpack {
    ($original:ty, $entity:ident) => {
        impl Unpack<$original> for packed::$entity {
            fn unpack(&self) -> $original {
                self.as_reader().unpack()
            }
        }
    };
}

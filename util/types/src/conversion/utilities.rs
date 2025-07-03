macro_rules! impl_conversion_for_entity_unpack {
    ($original:ty, $entity:ident) => {
        impl Unpack<$original> for packed::$entity {
            fn unpack(&self) -> $original {
                self.as_reader().unpack()
            }
        }
    };
}

macro_rules! impl_conversion_for_entity_from {
    ($original:ty, $entity:ident) => {
        impl From<packed::$entity> for $original {
            fn from(value: packed::$entity) -> $original {
                (&value).into()
            }
        }

        impl From<&packed::$entity> for $original {
            fn from(value: &packed::$entity) -> $original {
                value.as_reader().into()
            }
        }
    };
}

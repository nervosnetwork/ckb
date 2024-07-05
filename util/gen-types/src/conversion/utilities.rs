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
            fn from(v: packed::$entity) -> $original {
                (&v).into()
            }
        }

        impl From<&packed::$entity> for $original {
            fn from(v: &packed::$entity) -> $original {
                v.as_reader().into()
            }
        }
    };
}

macro_rules! impl_conversion_for_option_pack {
    ($original:ty, $entity:ident) => {
        impl Pack<packed::$entity> for Option<$original> {
            fn pack(&self) -> packed::$entity {
                if let Some(ref inner) = self {
                    packed::$entity::new_unchecked(inner.pack().as_bytes())
                } else {
                    packed::$entity::default()
                }
            }
        }
    };
}

macro_rules! impl_conversion_for_option_from {
    ($original:ty, $entity:ident, $entity_inner:ident) => {
        impl From<Option<$original>> for packed::$entity {
            fn from(value: Option<$original>) -> packed::$entity {
                if let Some(inner) = value {
                    packed::$entity::new_unchecked(
                        Into::<packed::$entity_inner>::into(inner).as_bytes(),
                    )
                } else {
                    packed::$entity::default()
                }
            }
        }
    };
}

macro_rules! impl_conversion_for_option_unpack {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl<'r> Unpack<Option<$original>> for packed::$reader<'r> {
            fn unpack(&self) -> Option<$original> {
                self.to_opt().map(|x| x.unpack())
            }
        }
        impl_conversion_for_entity_unpack!(Option<$original>, $entity);
    };
}

macro_rules! impl_conversion_for_option_into {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl<'r> Into<Option<$original>> for packed::$reader<'r> {
            fn into(self) -> Option<$original> {
                self.to_opt().map(|x| x.into())
            }
        }
        impl_conversion_for_entity_from!(Option<$original>, $entity);
    };
}

macro_rules! impl_conversion_for_option {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl_conversion_for_option_pack!($original, $entity);
        impl_conversion_for_option_unpack!($original, $entity, $reader);
    };
}

macro_rules! impl_conversion_for_option_from_into {
    ($original:ty, $entity:ident, $reader:ident, $entity_inner:ident) => {
        impl_conversion_for_option_from!($original, $entity, $entity_inner);
        impl_conversion_for_option_into!($original, $entity, $reader);
    };
}

macro_rules! impl_conversion_for_vector_pack {
    ($original:ty, $entity:ident) => {
        impl Pack<packed::$entity> for [$original] {
            fn pack(&self) -> packed::$entity {
                packed::$entity::new_builder()
                    .set(self.iter().map(|v| v.pack()).collect())
                    .build()
            }
        }
    };
}

macro_rules! impl_conversion_for_vector_from {
    ($original:ty, $entity:ident) => {
        impl From<&[$original]> for packed::$entity {
            fn from(value: &[$original]) -> packed::$entity {
                packed::$entity::new_builder()
                    .set(value.iter().map(|v| v.into()).collect())
                    .build()
            }
        }

        impl<const N: usize> From<[$original; N]> for packed::$entity {
            fn from(value: [$original; N]) -> packed::$entity {
                (&value[..]).into()
            }
        }

        impl<const N: usize> From<&[$original; N]> for packed::$entity {
            fn from(value: &[$original; N]) -> packed::$entity {
                (&value[..]).into()
            }
        }
    };
}

macro_rules! impl_conversion_for_vector_unpack {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl<'r> Unpack<Vec<$original>> for packed::$reader<'r> {
            fn unpack(&self) -> Vec<$original> {
                self.iter().map(|x| x.unpack()).collect()
            }
        }
        impl_conversion_for_entity_unpack!(Vec<$original>, $entity);
    };
}

macro_rules! impl_conversion_for_vector_into {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl<'r> Into<Vec<$original>> for packed::$reader<'r> {
            fn into(self) -> Vec<$original> {
                self.iter().map(|x| x.into()).collect()
            }
        }
        impl_conversion_for_entity_from!(Vec<$original>, $entity);
    };
}

macro_rules! impl_conversion_for_vector {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl_conversion_for_vector_pack!($original, $entity);
        impl_conversion_for_vector_unpack!($original, $entity, $reader);
    };
}

macro_rules! impl_conversion_for_vector_from_into {
    ($original:ty, $entity:ident, $reader:ident) => {
        impl_conversion_for_vector_from!($original, $entity);
        impl_conversion_for_vector_into!($original, $entity, $reader);
    };
}

macro_rules! impl_conversion_for_packed_optional_pack {
    ($original:ident, $entity:ident) => {
        impl Pack<packed::$entity> for Option<packed::$original> {
            fn pack(&self) -> packed::$entity {
                if let Some(ref inner) = self {
                    packed::$entity::new_unchecked(inner.as_bytes())
                } else {
                    packed::$entity::default()
                }
            }
        }
    };
}

macro_rules! impl_conversion_for_packed_optional_from {
    ($original:ident, $entity:ident) => {
        impl From<Option<packed::$original>> for packed::$entity {
            fn from(value: Option<packed::$original>) -> packed::$entity {
                if let Some(ref inner) = value {
                    packed::$entity::new_unchecked(inner.as_bytes())
                } else {
                    packed::$entity::default()
                }
            }
        }
    };
}

macro_rules! impl_conversion_for_packed_iterator_pack {
    ($item:ident, $vec:ident) => {
        impl<T> PackVec<packed::$vec, packed::$item> for T
        where
            T: IntoIterator<Item = packed::$item>,
        {
            fn pack(self) -> packed::$vec {
                packed::$vec::new_builder().extend(self).build()
            }
        }
    };
}

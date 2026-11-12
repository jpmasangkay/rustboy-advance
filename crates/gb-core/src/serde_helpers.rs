use alloc::{boxed::Box, vec::Vec};
use core::convert::TryInto;
use serde::{
    Deserializer, Serializer,
    de::{self, SeqAccess, Visitor},
};

/// Allocates a fixed-size boxed array directly on the heap, avoiding stack overflows
/// for large emulator memory buffers.
pub fn alloc_boxed_array<T: Clone, const N: usize>(val: T) -> Box<[T; N]> {
    let vec = alloc::vec![val; N];
    vec.into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("failed to convert boxed slice to array"))
}

pub mod box_bytes {
    use super::*;

    pub fn serialize<const N: usize, S: Serializer>(
        val: &[u8; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&val[..])
    }

    pub fn deserialize<'de, const N: usize, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<[u8; N]>, D::Error> {
        struct BoxBytesVisitor<const N: usize>;

        impl<'de, const N: usize> Visitor<'de> for BoxBytesVisitor<N> {
            type Value = Box<[u8; N]>;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "byte buffer of exact length {N}")
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                if v.len() != N {
                    return Err(E::invalid_length(v.len(), &self));
                }
                let boxed = v.to_vec().into_boxed_slice();
                boxed.try_into().map_err(|_| E::custom("length mismatch"))
            }

            fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                if v.len() != N {
                    return Err(E::invalid_length(v.len(), &self));
                }
                v.into_boxed_slice()
                    .try_into()
                    .map_err(|_| E::custom("length mismatch"))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut vec = Vec::with_capacity(N);
                while let Some(b) = seq.next_element::<u8>()? {
                    vec.push(b);
                }
                if vec.len() != N {
                    return Err(de::Error::invalid_length(vec.len(), &self));
                }
                vec.into_boxed_slice()
                    .try_into()
                    .map_err(|_| de::Error::custom("length mismatch"))
            }
        }

        deserializer.deserialize_bytes(BoxBytesVisitor::<N>)
    }
}

pub mod box_u16s {
    use super::*;

    pub fn serialize<const N: usize, S: Serializer>(
        val: &[u16; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(val.iter())
    }

    pub fn deserialize<'de, const N: usize, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<[u16; N]>, D::Error> {
        struct BoxU16sVisitor<const N: usize>;

        impl<'de, const N: usize> Visitor<'de> for BoxU16sVisitor<N> {
            type Value = Box<[u16; N]>;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "u16 sequence of exact length {N}")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut vec = Vec::with_capacity(N);
                while let Some(v) = seq.next_element::<u16>()? {
                    vec.push(v);
                }
                if vec.len() != N {
                    return Err(de::Error::invalid_length(vec.len(), &self));
                }
                vec.into_boxed_slice()
                    .try_into()
                    .map_err(|_| de::Error::custom("length mismatch"))
            }
        }

        deserializer.deserialize_seq(BoxU16sVisitor::<N>)
    }
}

pub mod fixed_bytes {
    use super::*;

    pub fn serialize<const N: usize, S: Serializer>(
        val: &[u8; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&val[..])
    }

    pub fn deserialize<'de, const N: usize, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[u8; N], D::Error> {
        struct FixedBytesVisitor<const N: usize>;

        impl<'de, const N: usize> Visitor<'de> for FixedBytesVisitor<N> {
            type Value = [u8; N];

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "byte array of exact length {N}")
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                v.try_into()
                    .map_err(|_| E::invalid_length(v.len(), &self))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut arr = [0u8; N];
                for (i, slot) in arr.iter_mut().enumerate() {
                    *slot = seq
                        .next_element::<u8>()?
                        .ok_or_else(|| de::Error::invalid_length(i, &self))?;
                }
                if seq.next_element::<u8>()?.is_some() {
                    return Err(de::Error::invalid_length(N + 1, &self));
                }
                Ok(arr)
            }
        }

        deserializer.deserialize_bytes(FixedBytesVisitor::<N>)
    }
}

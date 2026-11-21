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
                    if vec.len() >= N {
                        return Err(de::Error::invalid_length(N + 1, &self));
                    }
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
        #[cfg(target_endian = "little")]
        {
            // SAFETY: `u16` has no padding and every bit pattern is valid; on little-endian
            // targets its in-memory representation is identical to packed LE bytes.
            let bytes = unsafe { core::slice::from_raw_parts(val.as_ptr().cast::<u8>(), N * 2) };
            serializer.serialize_bytes(bytes)
        }
        #[cfg(not(target_endian = "little"))]
        {
            let mut bytes = Vec::with_capacity(N * 2);
            for &v in val {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            serializer.serialize_bytes(&bytes)
        }
    }

    pub fn deserialize<'de, const N: usize, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<[u16; N]>, D::Error> {
        struct BoxU16sVisitor<const N: usize>;

        impl<'de, const N: usize> Visitor<'de> for BoxU16sVisitor<N> {
            type Value = Box<[u16; N]>;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "little-endian u16 buffer of exact length {N}")
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                if v.len() != N * 2 {
                    return Err(E::invalid_length(v.len(), &self));
                }
                let mut out = alloc_boxed_array::<u16, N>(0);
                for (dst, chunk) in out.iter_mut().zip(v.as_chunks::<2>().0) {
                    *dst = u16::from_le_bytes(*chunk);
                }
                Ok(out)
            }

            fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                self.visit_bytes(&v)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut vec = Vec::with_capacity(N);
                while let Some(v) = seq.next_element::<u16>()? {
                    if vec.len() >= N {
                        return Err(de::Error::invalid_length(N + 1, &self));
                    }
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

        deserializer.deserialize_bytes(BoxU16sVisitor::<N>)
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
                v.try_into().map_err(|_| E::invalid_length(v.len(), &self))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
    struct SampleState {
        #[serde(with = "box_bytes")]
        bytes: Box<[u8; 16]>,
        #[serde(with = "box_u16s")]
        words: Box<[u16; 8]>,
        #[serde(with = "fixed_bytes")]
        fixed: [u8; 4],
    }

    #[test]
    fn serde_helpers_roundtrip_and_reject_truncated_payloads() {
        let mut bytes = alloc_boxed_array::<u8, 16>(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i as u8) ^ 0xA5;
        }
        let mut words = alloc_boxed_array::<u16, 8>(0);
        for (i, w) in words.iter_mut().enumerate() {
            *w = 0x1234u16.wrapping_add((i as u16) * 0x1111);
        }
        let state = SampleState {
            bytes,
            words,
            fixed: [0xDE, 0xAD, 0xBE, 0xEF],
        };
        let encoded = postcard::to_allocvec(&state).expect("serialize sample state");
        let decoded: SampleState =
            postcard::from_bytes(&encoded).expect("deserialize sample state");
        assert_eq!(decoded, state);

        // Truncated buffer must fail cleanly without panicking.
        assert!(postcard::from_bytes::<SampleState>(&encoded[..encoded.len() - 3]).is_err());
    }
}

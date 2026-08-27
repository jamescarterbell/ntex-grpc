#![allow(
    clippy::cast_lossless,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
use std::hash::BuildHasher;
use std::{collections::HashMap, convert::TryFrom, fmt, mem, sync::Arc};

use ntex_bytes::{Buf, BufMut, BytePages, ByteString, Bytes};
use ntex_util::hash_map::HashMap as HashMapBase;

pub use crate::encoding::WireType;
use crate::encoding::{self, DecodeError};

/// Protobuf struct read/write operations
pub trait Message<'m>: Default + Sized + fmt::Debug {
    type InputView<'a>: Message<'a, InputView<'a> = Self::InputView<'a>, OutputView<'a> = Self::OutputView<'a>>
        + PartialEq
        + 'a;
    type OutputView<'a>: Message<'a, InputView<'a> = Self::InputView<'a>, OutputView<'a> = Self::OutputView<'a>>
        + PartialEq
        + 'a;

    /// Decodes an instance of the message from a buffer
    fn read(src: &'m mut Bytes) -> Result<Self, DecodeError>;

    /// Encodes and writes the message to a buffer
    fn write(&self, dst: &mut BytePages);

    /// Returns the encoded length of the message with a length delimiter
    fn encoded_len(&self) -> usize;
}

/// Default type value
pub enum DefaultValue<T> {
    Unknown,
    Default,
    Value(T),
}

/// Protobuf type serializer
pub trait NativeType<'m>: PartialEq + Default + Sized + fmt::Debug {
    const TYPE: WireType;

    /// The `+ 'a` bound makes `Self::InputView<'a>: 'a` known at every use site,
    /// which is what allows borrowed views such as `&'a [T::InputView<'a>]` to
    /// be well-formed.
    type InputView<'a>: NativeType<'a, InputView<'a> = Self::InputView<'a>> + 'a;
    type OutputView<'a>: NativeType<'a, OutputView<'a> = Self::OutputView<'a>> + 'a;

    #[inline]
    /// Returns the encoded length of the message without a length delimiter.
    fn value_len(&self) -> usize {
        0
    }

    /// Deserialize from the input
    fn merge(src: &'m mut Bytes) -> Result<Self, DecodeError>;

    /// Check if value is default
    fn is_default(&self) -> bool {
        false
    }

    /// Encode field value
    fn encode_value(&self, dst: &mut BytePages);

    #[inline]
    /// Encode field tag and length
    fn encode_type(&self, tag: u32, dst: &mut BytePages) {
        encoding::encode_key(tag, Self::TYPE, dst);
        if !matches!(Self::TYPE, WireType::Varint | WireType::SixtyFourBit) {
            encoding::encode_varint(self.value_len() as u64, dst);
        }
    }

    #[inline]
    /// Protobuf field length
    fn encoded_len(&self, tag: u32) -> usize {
        let value_len = self.value_len();
        encoding::key_len(tag) + encoding::encoded_len_varint(value_len as u64) + value_len
    }

    #[inline]
    /// Serialize protobuf field
    fn serialize(&self, tag: u32, default: DefaultValue<&Self>, dst: &mut BytePages) {
        let default = match default {
            DefaultValue::Unknown => false,
            DefaultValue::Default => self.is_default(),
            DefaultValue::Value(d) => self == d,
        };

        if !default {
            self.encode_type(tag, dst);
            self.encode_value(dst);
        }
    }

    #[inline]
    /// Protobuf field length
    fn serialized_len(&self, tag: u32, default: DefaultValue<&Self>) -> usize {
        let default = match default {
            DefaultValue::Unknown => false,
            DefaultValue::Default => self.is_default(),
            DefaultValue::Value(d) => self == d,
        };

        if default { 0 } else { self.encoded_len(tag) }
    }

    #[inline]
    /// Deserialize protobuf field
    fn deserialize(_: u32, wtype: WireType, src: &'m mut Bytes) -> Result<Self, DecodeError> {
        encoding::check_wire_type(Self::TYPE, wtype)?;

        if matches!(Self::TYPE, WireType::Varint | WireType::SixtyFourBit) {
            Self::merge(src)
        } else {
            let len = encoding::decode_varint(src)? as usize;
            
            let mut buf = src.split_to_checked(len).ok_or_else(|| {
                DecodeError::new(format!(
                    "Not enough data, message size {} buffer size {}",
                    len,
                    src.len()
                ))
            })?;
            Self::merge(&mut buf)
        }
    }

    // #[inline]
    // /// Deserialize protobuf field to default value
    // fn deserialize_default(
    //     tag: u32,
    //     wtype: WireType,
    //     src: &mut Bytes,
    // ) -> Result<Self, DecodeError> {
    //     Self::deserialize(tag, wtype, src)
    // }
}

/// Protobuf struct read/write operations
impl<'m> Message<'m> for () {
    type InputView<'a> = ();
    type OutputView<'a> = ();

    fn encoded_len(&self) -> usize {
        0
    }

    fn read(_: &mut Bytes) -> Result<Self, DecodeError> {
        Ok(())
    }

    fn write(&self, _: &mut BytePages) {}
}

impl<'m, T: Message<'m> + PartialEq> NativeType<'m> for T {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = <T as Message<'m>>::InputView<'a>;
    type OutputView<'a> = <T as Message<'m>>::OutputView<'a>;

    fn value_len(&self) -> usize {
        Message::encoded_len(self)
    }

    #[inline]
    /// Encode message to the buffer
    fn encode_value(&self, dst: &mut BytePages) {
        self.write(dst);
    }

    /// Deserialize from the input
    fn merge(src: &'m mut Bytes) -> Result<Self, DecodeError> {
        Message::read(src)
    }
}

impl NativeType<'_> for Bytes {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a [u8];
    type OutputView<'a> = &'a [u8];

    #[inline]
    fn value_len(&self) -> usize {
        self.len()
    }

    #[inline]
    /// Serialize field value
    fn encode_value(&self, dst: &mut BytePages) {
        dst.append(self.clone());
    }

    #[inline]
    /// Deserialize from the input
    fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
        Ok(mem::take(src))
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl<'m> NativeType<'m> for &'m [u8] {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a [u8];
    type OutputView<'a> = &'a [u8];

    #[inline]
    fn value_len(&self) -> usize {
        self.len()
    }

    #[inline]
    /// Serialize field value
    fn encode_value(&self, dst: &mut BytePages) {
        dst.append(self.clone());
    }

    #[inline]
    /// Deserialize from the input
    fn merge(src: &'m mut Bytes) -> Result<Self, DecodeError> {
        Ok(&src[..])
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl NativeType<'_> for String {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a str;
    type OutputView<'a> = &'a str;

    #[inline]
    fn value_len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
        if let Ok(s) = ByteString::try_from(mem::take(src)) {
            Ok(s.as_str().to_string())
        } else {
            Err(DecodeError::new(
                "invalid string value: data is not UTF-8 encoded",
            ))
        }
    }

    #[inline]
    fn encode_value(&self, dst: &mut BytePages) {
        dst.extend_from_slice(self.as_bytes());
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl<'m> NativeType<'m> for &'m str {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a str;
    type OutputView<'a> = &'a str;

    #[inline]
    fn value_len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
        if let Ok(s) = ByteString::try_from(mem::take(src)) {
            Ok(s.as_str().to_string())
        } else {
            Err(DecodeError::new(
                "invalid string value: data is not UTF-8 encoded",
            ))
        }
    }

    #[inline]
    fn encode_value(&self, dst: &mut BytePages) {
        dst.extend_from_slice(self.as_bytes());
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl NativeType<'_> for ByteString {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a str;
    type OutputView<'a> = &'a str;

    #[inline]
    fn value_len(&self) -> usize {
        self.as_slice().len()
    }

    #[inline]
    fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
        if let Ok(s) = ByteString::try_from(mem::take(src)) {
            Ok(s)
        } else {
            Err(DecodeError::new(
                "invalid string value: data is not UTF-8 encoded",
            ))
        }
    }

    #[inline]
    fn encode_value(&self, dst: &mut BytePages) {
        dst.append(self.clone());
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl NativeType<'_> for Arc<str> {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a str;
    type OutputView<'a> = &'a str;

    #[inline]
    fn value_len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
        if let Ok(s) = ByteString::try_from(mem::take(src)) {
            Ok(Arc::from(s.as_str()))
        } else {
            Err(DecodeError::new(
                "invalid string value: data is not UTF-8 encoded",
            ))
        }
    }

    #[inline]
    fn encode_value(&self, dst: &mut BytePages) {
        dst.extend_from_slice(self.as_bytes());
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl<'m, T: NativeType<'m>> NativeType<'m> for Option<T> {
    const TYPE: WireType = WireType::LengthDelimited;
    
    type InputView<'a> = Option<T::InputView<'a>>;
    type OutputView<'a> = Option<T::OutputView<'a>>;

    #[inline]
    fn is_default(&self) -> bool {
        self.is_none()
    }

    #[inline]
    /// Serialize field value
    fn encode_value(&self, _: &mut BytePages) {}

    #[inline]
    /// Deserialize from the input
    fn merge(_: &mut Bytes) -> Result<Self, DecodeError> {
        Err(DecodeError::new(
            "Cannot directly call deserialize for Option<T>",
        ))
    }

    #[inline]
    /// Deserialize protobuf field
    fn deserialize(tag: u32, wtype: WireType, src: &mut Bytes) -> Result<Self, DecodeError> {
        Ok(Some(T::deserialize(tag, wtype, src)?))
    }

    #[inline]
    /// Serialize protobuf field
    fn serialize(&self, tag: u32, _: DefaultValue<&Self>, dst: &mut BytePages) {
        if let Some(value) = self {
            value.serialize(tag, DefaultValue::Unknown, dst);
        }
    }

    #[inline]
    /// Protobuf field length
    fn serialized_len(&self, tag: u32, _: DefaultValue<&Self>) -> usize {
        if let Some(value) = self {
            value.serialized_len(tag, DefaultValue::Unknown)
        } else {
            0
        }
    }

    #[inline]
    /// Protobuf field length
    fn encoded_len(&self, tag: u32) -> usize {
        self.as_ref().map_or(0, |value| value.encoded_len(tag))
    }
}

impl NativeType<'_> for Vec<u8> {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a [u8];
    type OutputView<'a> = &'a [u8];

    #[inline]
    fn value_len(&self) -> usize {
        self.len()
    }

    #[inline]
    /// Serialize field value
    fn encode_value(&self, dst: &mut BytePages) {
        dst.extend_from_slice(self.as_slice());
    }

    #[inline]
    /// Deserialize from the input
    fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
        Ok(Vec::from(&src[..]))
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }
}

impl<'m, T: NativeType<'m>> NativeType<'m> for Vec<T> {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a [T::InputView<'a>];
    type OutputView<'a> = Vec<T::OutputView<'a>>;
    
    #[inline]
    /// Serialize field value
    fn encode_value(&self, _: &mut BytePages) {}

    #[inline]
    /// Deserialize from the input
    fn merge(_: &mut Bytes) -> Result<Self, DecodeError> {
        Err(DecodeError::new("Cannot directly call merge for Vec<T>"))
    }

    /// Deserialize protobuf field
    fn deserialize(tag: u32, wtype: WireType, src: &mut Bytes) -> Result<Self, DecodeError> {
        let mut vec = Vec::new();
        if T::TYPE == WireType::Varint {
            let len = encoding::decode_varint(src)? as usize;
            let mut buf = src
                .split_to_checked(len)
                .ok_or_else(DecodeError::incomplete)?;
            while !buf.is_empty() {
                vec.push(T::merge(&mut buf)?);
            }
        } else {
            vec.push(T::deserialize(tag, wtype, src)?);
        }
        Ok(vec)
    }

    /// Serialize protobuf field
    fn serialize(&self, tag: u32, _: DefaultValue<&Self>, dst: &mut BytePages) {
        if self.is_empty() {
            return;
        }
        if T::TYPE == WireType::Varint {
            encoding::encode_key(tag, WireType::LengthDelimited, dst);
            encoding::encode_varint(
                self.iter().map(NativeType::value_len).sum::<usize>() as u64,
                dst,
            );
            for item in self {
                item.encode_value(dst);
            }
        } else {
            for item in self {
                item.serialize(tag, DefaultValue::Unknown, dst);
            }
        }
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }

    /// Protobuf field length
    fn encoded_len(&self, tag: u32) -> usize {
        if T::TYPE == WireType::Varint {
            let len = self.iter().map(NativeType::value_len).sum::<usize>();
            self.iter().map(NativeType::value_len).sum::<usize>()
                + encoding::key_len(tag)
                + encoding::encoded_len_varint(len as u64)
        } else {
            self.iter().map(|value| value.encoded_len(tag)).sum()
        }
    }
}

impl<'m, T: NativeType<'m>> NativeType<'m> for &'m [T] {
    const TYPE: WireType = WireType::LengthDelimited;

    type InputView<'a> = &'a [T::InputView<'a>];
    type OutputView<'a> = Vec<T::OutputView<'a>>;
    
    #[inline]
    /// Serialize field value
    fn encode_value(&self, _: &mut BytePages) {}

    #[inline]
    /// Deserialize from the input
    fn merge(_: &mut Bytes) -> Result<Self, DecodeError> {
        Err(DecodeError::new("Cannot directly call merge for Vec<T>"))
    }

    /// Deserialize protobuf field
    fn deserialize(tag: u32, wtype: WireType, src: &mut Bytes) -> Result<Self, DecodeError> {
        let mut vec = Vec::new();
        if T::TYPE == WireType::Varint {
            let len = encoding::decode_varint(src)? as usize;
            let mut buf = src
                .split_to_checked(len)
                .ok_or_else(DecodeError::incomplete)?;
            while !buf.is_empty() {
                vec.push(T::merge(&mut buf)?);
            }
        } else {
            vec.push(T::deserialize(tag, wtype, src)?);
        }
        Ok(vec)
    }

    /// Serialize protobuf field
    fn serialize(&self, tag: u32, _: DefaultValue<&Self>, dst: &mut BytePages) {
        if self.is_empty() {
            return;
        }
        if T::TYPE == WireType::Varint {
            encoding::encode_key(tag, WireType::LengthDelimited, dst);
            encoding::encode_varint(
                self.iter().map(NativeType::value_len).sum::<usize>() as u64,
                dst,
            );
            for item in self {
                item.encode_value(dst);
            }
        } else {
            for item in self {
                item.serialize(tag, DefaultValue::Unknown, dst);
            }
        }
    }

    #[inline]
    fn is_default(&self) -> bool {
        self.is_empty()
    }

    /// Protobuf field length
    fn encoded_len(&self, tag: u32) -> usize {
        if T::TYPE == WireType::Varint {
            let len = self.iter().map(NativeType::value_len).sum::<usize>();
            self.iter().map(NativeType::value_len).sum::<usize>()
                + encoding::key_len(tag)
                + encoding::encoded_len_varint(len as u64)
        } else {
            self.iter().map(|value| value.encoded_len(tag)).sum()
        }
    }
}

macro_rules! hashmap {
    ($ty:ident, $($key:ty),* $(,)?) => {$(
        impl<'m, V: NativeType<'m>, S: BuildHasher + Default + 'static>
            NativeType<'m> for $ty<$key, V, S>
        {
            const TYPE: WireType = WireType::LengthDelimited;

            type InputView<'a> = $ty<<$key as NativeType<'m>>::InputView<'a>, V::InputView<'a>, S>;
            type OutputView<'a> = $ty<<$key as NativeType<'m>>::OutputView<'a>, V::OutputView<'a>, S>;

            #[inline]
            /// Deserialize from the input
            fn merge(_: &mut Bytes) -> Result<Self, DecodeError> {
                Err(DecodeError::new("Cannot directly call merge for Map<K, V>"))
            }

            #[inline]
            /// Serialize field value
            fn encode_value(&self, _: &mut BytePages) {}

            #[inline]
            fn is_default(&self) -> bool {
                self.is_empty()
            }

            /// Deserialize protobuf field
            fn deserialize(_: u32, wtype: WireType, src: &mut Bytes) -> Result<Self, DecodeError> {
                encoding::check_wire_type(Self::TYPE, wtype)?;

                let len = encoding::decode_varint(src)? as usize;
                let mut buf = src.split_to_checked(len).ok_or_else(|| {
                    DecodeError::new(format!(
                        "Not enough data for HashMap, message size {}, buf size {}",
                        len,
                        src.len()
                    ))
                })?;
                let mut key = Default::default();
                let mut val = Default::default();

                while !buf.is_empty() {
                    let (tag, wire_type) = encoding::decode_key(&mut buf)?;
                    match tag {
                        1 => key = <$key as NativeType<'m>>::deserialize(1, wire_type, &mut buf)?,
                        2 => val = V::deserialize(2, wire_type, &mut buf)?,
                        _ => return Err(DecodeError::new("Map deserialization error")),
                    }
                }
                let mut map = Self::default();
                map.insert(key, val);
                Ok(map)
            }

            /// Serialize protobuf field
            fn serialize(&self, tag: u32, _: DefaultValue<&Self>, dst: &mut BytePages) {
                let key_default = <$key as Default>::default();
                let val_default = V::default();

                for item in self {
                    let skip_key = item.0 == &key_default;
                    let skip_val = item.1 == &val_default;

                    let len = (if skip_key { 0 } else { item.0.encoded_len(1) })
                        + (if skip_val { 0 } else { item.1.encoded_len(2) });

                    encoding::encode_key(tag, WireType::LengthDelimited, dst);
                    encoding::encode_varint(len as u64, dst);
                    if !skip_key {
                        item.0.serialize(1, DefaultValue::Default, dst);
                    }
                    if !skip_val {
                        item.1.serialize(2, DefaultValue::Default, dst);
                    }
                }
            }

            /// Generic protobuf map encode function with an overridden value default.
            fn encoded_len(&self, tag: u32) -> usize {
                let key_default = <$key as Default>::default();
                let val_default = V::default();

                self.iter()
                    .map(|(key, val)| {
                        let len = (if key == &key_default {
                            0
                        } else {
                            key.encoded_len(1)
                        }) + (if val == &val_default {
                            0
                        } else {
                            val.encoded_len(2)
                        });

                        encoding::key_len(tag) + encoding::encoded_len_varint(len as u64) + len
                    })
                    .sum::<usize>()
            }
        }
    )*};
}

hashmap!(HashMap, bool, i32, i64, u32, u64, String, ByteString, Arc<str>);
hashmap!(HashMapBase, bool, i32, i64, u32, u64, String, ByteString, Arc<str>);

/// The view of every owned string key spelling is `&'a str`, so the borrowed-key
/// maps need their own impls for the view fixed point to close.
macro_rules! hashmap_str_key {
    ($($ty:ident),* $(,)?) => {$(
        impl<'m, V: NativeType<'m>, S: BuildHasher + Default + 'static> NativeType<'m>
            for $ty<&'m str, V, S>
        {
            const TYPE: WireType = WireType::LengthDelimited;

            type InputView<'a> = $ty<&'a str, V::InputView<'a>, S>;
            type OutputView<'a> = $ty<&'a str, V::OutputView<'a>, S>;

            #[inline]
            fn merge(_: &mut Bytes) -> Result<Self, DecodeError> {
                Err(DecodeError::new("Cannot directly call merge for Map<K, V>"))
            }

            #[inline]
            fn encode_value(&self, _: &mut BytePages) {}

            #[inline]
            fn is_default(&self) -> bool {
                self.is_empty()
            }
        }
    )*};
}

hashmap_str_key!(HashMap, HashMapBase);

/// Macro which emits a module containing a set of encoding functions for a
/// variable width numeric type.
macro_rules! varint {
    ($ty:ident, $default:expr) => (
        varint!($ty, $default, to_uint64(self) { *self as u64 }, from_uint64(v) { v as $ty });
    );

    ($ty:ty, $default:expr, to_uint64($slf:ident) $to_uint64:expr, from_uint64($val:ident) $from_uint64:expr) => (

        impl<'m> NativeType<'m> for $ty {
            const TYPE: WireType = WireType::Varint;

            type InputView<'a> = Self;
            type OutputView<'a> = Self;

            #[inline]
            fn is_default(&self) -> bool {
                *self == $default
            }

            #[inline]
            fn encode_value(&$slf, dst: &mut BytePages) {
                encoding::encode_varint($to_uint64, dst);
            }

            #[inline]
            fn encoded_len(&$slf, tag: u32) -> usize {
                encoding::key_len(tag) + encoding::encoded_len_varint($to_uint64)
            }

            #[inline]
            fn value_len(&$slf) -> usize {
                encoding::encoded_len_varint($to_uint64)
            }

            #[inline]
            fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
                encoding::decode_varint(src).map(|$val| $from_uint64)
            }
        }
    );
}

varint!(bool, false,
        to_uint64(self) u64::from(*self),
        from_uint64(value) value != 0);
varint!(i32, 0i32);
varint!(i64, 0i64);
varint!(u32, 0u32);
varint!(u64, 0u64);

/// Macro which emits a module containing a set of encoding functions for a
/// fixed width numeric type.
macro_rules! fixed_width {
    ($ty:ty,
     $width:expr,
     $wire_type:expr,
     $default:expr,
     $put:expr,
     $get:expr) => {
        impl<'m> NativeType<'m> for $ty {
            const TYPE: WireType = $wire_type;

            type InputView<'a> = Self;
            type OutputView<'a> = Self;

            #[inline]
            fn is_default(&self) -> bool {
                *self == $default
            }

            #[inline]
            fn encode_value(&self, dst: &mut BytePages) {
                $put(dst, *self);
            }

            #[inline]
            fn encoded_len(&self, tag: u32) -> usize {
                encoding::key_len(tag) + $width
            }

            #[inline]
            fn value_len(&self) -> usize {
                $width
            }

            #[inline]
            fn merge(src: &mut Bytes) -> Result<Self, DecodeError> {
                if src.len() < $width {
                    return Err(DecodeError::new("Buffer underflow"));
                }
                let value = $get(src);
                Ok(value)
            }
        }
    };
}

fixed_width!(
    f32,
    4,
    WireType::ThirtyTwoBit,
    0f32,
    BufMut::put_f32_le,
    Buf::get_f32_le
);
fixed_width!(
    f64,
    8,
    WireType::SixtyFourBit,
    0f64,
    BufMut::put_f64_le,
    Buf::get_f64_le
);

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug, Default)]
    pub struct TestMessage {
        f: f64,
        props: HashMap<String, u32>,
        b: bool,
        opt: Option<String>,
    }

    #[derive(Clone, PartialEq, Debug, Default)]
    pub struct TestMessageInputView<'a> {
        f: f64,
        props: HashMap<String, u32>,
        b: bool,
        opt: Option<&'a str>,
    }
    
    #[derive(Clone, PartialEq, Debug, Default)]
    pub struct TestMessageOutputView<'a> {
        f: f64,
        props: HashMap<String, u32>,
        b: bool,
        opt: Option<&'a str>,
    }

    impl<'m> Message<'m> for TestMessage {
    
        type InputView<'a> = TestMessageInputView<'a>;
        type OutputView<'a> = TestMessageOutputView<'a>;

        fn write(&self, dst: &mut BytePages) {
            NativeType::serialize(&self.f, 1, DefaultValue::Default, dst);
            NativeType::serialize(&self.props, 2, DefaultValue::Default, dst);
            NativeType::serialize(&self.b, 3, DefaultValue::Default, dst);
            NativeType::serialize(&self.opt, 4, DefaultValue::Default, dst);
        }

        #[inline]
        fn read(src: &mut Bytes) -> Result<Self, DecodeError> {
            let mut msg = Self::default();
            while !src.is_empty() {
                let (tag, wire_type) = encoding::decode_key(src)?;
                match tag {
                    1 => msg.f = NativeType::deserialize(tag, wire_type, src)?,
                    2 => msg.props = NativeType::deserialize(tag, wire_type, src)?,
                    3 => msg.b = NativeType::deserialize(tag, wire_type, src)?,
                    4 => msg.opt = NativeType::deserialize(tag, wire_type, src)?,
                    _ => encoding::skip_field(wire_type, tag, src)?,
                }
            }
            Ok(msg)
        }

        #[allow(clippy::identity_op)]
        #[inline]
        fn encoded_len(&self) -> usize {
            0 + NativeType::serialized_len(&self.f, 1, DefaultValue::Default)
                + NativeType::serialized_len(&self.props, 2, DefaultValue::Default)
                + NativeType::serialized_len(&self.b, 3, DefaultValue::Default)
                + NativeType::serialized_len(&self.opt, 4, DefaultValue::Default)
        }
    }

    impl<'m> Message<'m> for TestMessageInputView<'m> {
    
        type InputView<'a> = TestMessageInputView<'a>;
        type OutputView<'a> = TestMessageOutputView<'a>;

        fn write(&self, dst: &mut BytePages) {
            NativeType::serialize(&self.f, 1, DefaultValue::Default, dst);
            NativeType::serialize(&self.props, 2, DefaultValue::Default, dst);
            NativeType::serialize(&self.b, 3, DefaultValue::Default, dst);
            NativeType::serialize(&self.opt, 4, DefaultValue::Default, dst);
        }

        #[inline]
        fn read(src: &mut Bytes) -> Result<Self, DecodeError> {
            let mut msg = Self::default();
            while !src.is_empty() {
                let (tag, wire_type) = encoding::decode_key(src)?;
                match tag {
                    1 => msg.f = NativeType::deserialize(tag, wire_type, src)?,
                    2 => msg.props = NativeType::deserialize(tag, wire_type, src)?,
                    3 => msg.b = NativeType::deserialize(tag, wire_type, src)?,
                    4 => msg.opt = NativeType::deserialize(tag, wire_type, src)?,
                    _ => encoding::skip_field(wire_type, tag, src)?,
                }
            }
            Ok(msg)
        }

        #[allow(clippy::identity_op)]
        #[inline]
        fn encoded_len(&self) -> usize {
            0 + NativeType::serialized_len(&self.f, 1, DefaultValue::Default)
                + NativeType::serialized_len(&self.props, 2, DefaultValue::Default)
                + NativeType::serialized_len(&self.b, 3, DefaultValue::Default)
                + NativeType::serialized_len(&self.opt, 4, DefaultValue::Default)
        }
    }

    impl<'m> Message<'m> for TestMessageOutputView<'m> {
    
        type InputView<'a> = TestMessageInputView<'a>;
        type OutputView<'a> = TestMessageOutputView<'a>;

        fn write(&self, dst: &mut BytePages) {
            NativeType::serialize(&self.f, 1, DefaultValue::Default, dst);
            NativeType::serialize(&self.props, 2, DefaultValue::Default, dst);
            NativeType::serialize(&self.b, 3, DefaultValue::Default, dst);
            NativeType::serialize(&self.opt, 4, DefaultValue::Default, dst);
        }

        #[inline]
        fn read(src: &mut Bytes) -> Result<Self, DecodeError> {
            let mut msg = Self::default();
            while !src.is_empty() {
                let (tag, wire_type) = encoding::decode_key(src)?;
                match tag {
                    1 => msg.f = NativeType::deserialize(tag, wire_type, src)?,
                    2 => msg.props = NativeType::deserialize(tag, wire_type, src)?,
                    3 => msg.b = NativeType::deserialize(tag, wire_type, src)?,
                    4 => msg.opt = NativeType::deserialize(tag, wire_type, src)?,
                    _ => encoding::skip_field(wire_type, tag, src)?,
                }
            }
            Ok(msg)
        }

        #[allow(clippy::identity_op)]
        #[inline]
        fn encoded_len(&self) -> usize {
            0 + NativeType::serialized_len(&self.f, 1, DefaultValue::Default)
                + NativeType::serialized_len(&self.props, 2, DefaultValue::Default)
                + NativeType::serialized_len(&self.b, 3, DefaultValue::Default)
                + NativeType::serialized_len(&self.opt, 4, DefaultValue::Default)
        }
    }

    #[allow(clippy::field_reassign_with_default)]
    #[test]
    fn test_hashmap_default_values() {
        let mut msg = TestMessage::default();

        msg.f = 382.8263;
        msg.b = true;
        msg.props.insert("test1".to_string(), 1);
        msg.props.insert("test2".to_string(), 0);
        msg.props.insert(String::new(), 0);

        let mut buf = BytePages::default();
        msg.write(&mut buf);
        assert_eq!(Message::encoded_len(&msg), 33);
        assert_eq!(buf.len(), 33);

        let mut buf2 = BytePages::default();
        msg.serialize(1, DefaultValue::Default, &mut buf2);
        assert_eq!(NativeType::encoded_len(&msg, 1), 35);
        assert_eq!(buf2.len(), 35);

        let msg2 = TestMessage::read(&mut buf.freeze()).unwrap();
        assert_eq!(Message::encoded_len(&msg2), 33);
        assert_eq!(msg, msg2);

        let mut buf2 = buf2.freeze();
        let (tag, wire_type) = encoding::decode_key(&mut buf2).unwrap();
        let msg3 = TestMessage::deserialize(tag, wire_type, &mut buf2).unwrap();
        assert_eq!(msg, msg3);
    }
}

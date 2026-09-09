use ntex_bytes::{BytePages, ByteString, Bytes};

use crate::{encoding::DecodeError, server::MethodResult, types::Message};

/// Trait for service method definition
pub trait ServiceDef {
    const NAME: &'static str;

    type Methods;

    fn method_by_name(name: &str) -> Option<Self::Methods>;
}

/// Trait for service method definition
pub trait MethodDef {
    const NAME: &'static str;

    const PATH: ByteString;

    /// The owned variant of the input message.
    type Input: for<'i> Message<'i>;

    /// The owned variant of the output message.
    type Output: for<'o> Message<'o>;

    #[inline]
    fn decode<'m>(&'m self, buf: &mut Bytes) -> Result<Self::Input, DecodeError> {
        Message::read(buf)
    }

    #[inline]
    fn encode(&self, val: Self::Output, buf: &mut BytePages) {
        val.write(buf);
    }

    #[doc(hidden)]
    #[inline]
    fn server_result<'m, T: MethodResult<Self::Output>>(&'m self, val: T) -> Self::Output {
        val.into()
    }
}

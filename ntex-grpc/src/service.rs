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

    type Input<'i>: Message<'i> where Self: 'i;

    type Output<'o>: Message<'o>;

    #[inline]
    fn decode<'m>(&'m self, buf: &mut Bytes) -> Result<Self::Input<'m>, DecodeError> {
        Message::read(buf)
    }

    #[inline]
    fn encode(&self, val: Self::Output<'_>, buf: &mut BytePages) {
        val.write(buf);
    }

    #[doc(hidden)]
    #[inline]
    fn server_result<'m, T: MethodResult<Self::Output<'m>>>(&'m self, val: T) -> Self::Output<'m> {
        val.into()
    }
}

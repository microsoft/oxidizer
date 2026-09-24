// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Encoded component grammar used by validated construction.

use super::invalid;
use crate::DecodeError;

// fluent-uri has no stable component validators. Checking these RFC 3986
// productions directly avoids assembling a URI just to parse it again.
#[derive(Clone, Copy, Debug)]
pub(super) enum Component {
    Scheme,
    Userinfo,
    RegisteredName,
    Path,
    QueryFragment,
    IpvFuture,
}

impl Component {
    pub(super) fn validate(self, text: &str) -> Result<(), DecodeError> {
        let mut bytes = text.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'%' && !matches!(self, Self::Scheme | Self::IpvFuture) {
                if !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit()) || !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(invalid());
                }
            } else if !self.allows(byte) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn allows(self, byte: u8) -> bool {
        if byte.is_ascii_alphanumeric() {
            return true;
        }
        if matches!(self, Self::Scheme) {
            return matches!(byte, b'+' | b'-' | b'.');
        }
        if matches!(
            byte,
            b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
        ) {
            return true;
        }
        match self {
            Self::Userinfo | Self::IpvFuture => byte == b':',
            Self::Path => matches!(byte, b':' | b'@' | b'/'),
            Self::QueryFragment => matches!(byte, b':' | b'@' | b'/' | b'?'),
            Self::Scheme | Self::RegisteredName => false,
        }
    }
}

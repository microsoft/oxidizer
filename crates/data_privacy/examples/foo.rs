// Copyright (c) Microsoft Corporation.

// Changes TODO:
// - classified() impls for wrapper only, but also does derive logic
// - #[derive(RedactedDebug, RedactedDisplay, RedactedToString) ... aka Redacted)] field by field
// - have public / unknown(?) data class for `String` and co -- types that have valid Debug / Display but may contain whatever data
// - introduce data_privacy_macros_impl crate and move testing

use data_privacy::Classified;
use data_privacy_macros::{classified, taxonomy};

#[taxonomy(example)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Tax {
    PII,
    OII,
}

// #[classified(Tax::PII)]
// struct Personal(String);

fn main() {

}

//
// #[derive(
//     data_privacy_macros::ClassifiedDebug,
//     data_privacy_macros::RedactedDebug,
//     data_privacy_macros::RedactedDisplay,
//     data_privacy_macros::RedactedToString
// )]
// struct Personal(String);
//
// impl core::fmt::Debug for Personal { fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { f.write_fmt(format_args!("<CLASSIFIED:{}/{}>", data_privacy::Classified::data_class(self).taxonomy(), data_privacy::Classified::data_class(self).name())) } }
//
//
// impl data_privacy::RedactedDebug for Personal {
//     #[expect(
//         clippy::cast_possible_truncation,
//         reason = "Converting from u64 to usize, value is known to be <= 128"
//     )]
//     fn fmt(&self, engine: &data_privacy::RedactionEngine, output: &mut std::fmt::Formatter<'_>) -> core::fmt::Result {
//         let v = self.0;
//         let mut local_buf = [0u8; 128];
//         let amount = {
//             let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
//             if std::io::Write::write_fmt(&mut cursor, format_args!("{v:?}", v = v)).is_ok() { cursor.position() as usize } else { local_buf.len() + 1 }
//         };
//         if amount <= local_buf.len() {
//             let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };
//             engine.redact(&self.data_class(), s, output)
//         } else {
//             engine.redact(&self.data_class(), ::alloc::__export::must_use({
//                 ::alloc::fmt::format(format_args!("{v:?}", v = v))
//             }), output)
//         }
//     }
// }
//
// impl data_privacy::RedactedDisplay for Personal {
//     #[expect(
//         clippy::cast_possible_truncation,
//         reason = "Converting from u64 to usize, value is known to be <= 128"
//     )]
//     fn fmt(&self, engine: &data_privacy::RedactionEngine, output: &mut std::fmt::Formatter) -> core::fmt::Result {
//         let v = self.as_declassified();
//         let mut local_buf = [0u8; 128];
//         let amount = {
//             let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
//             if std::io::Write::write_fmt(&mut cursor, format_args!("{v}", v = v)).is_ok() { cursor.position() as usize } else { local_buf.len() + 1 }
//         };
//         if amount <= local_buf.len() {
//             let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };
//             engine.redact(&self.data_class(), s, output)
//         } else {
//             engine.redact(&self.data_class(), ::alloc::__export::must_use({
//                 ::alloc::fmt::format(format_args!("{v}", v = v))
//             }), output)
//         }
//     }
// }
//
// impl data_privacy::RedactedToString for Personal {
//     fn to_string(&self, engine: &data_privacy::RedactionEngine) -> String {
//         let v = self.as_declassified();
//         let mut output = String::new();
//         _ = engine.redact(&self.data_class(), v.to_string(), &mut output);
//         output
//     }
// }
//
// impl data_privacy::Classified for Personal {
//     fn data_class(&self) -> data_privacy::DataClass { Self::data_class(self) }
// }
//
// impl core::ops::Deref for Personal {
//     type Target = core::convert::Infallible;
//     fn deref(&self) -> &Self::Target { todo!() }
// }
//
// impl core::ops::DerefMut for Personal { fn deref_mut(&mut self) -> &mut Self::Target { todo!() } }
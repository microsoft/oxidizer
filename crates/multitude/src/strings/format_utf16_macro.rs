// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// `format!`-style macro that writes into a fresh arena-backed
/// [`Utf16String`](crate::strings::Utf16String).
///
/// Freeze the result with [`Utf16String::into_boxed_utf16_str`](crate::strings::Utf16String::into_boxed_utf16_str)
/// if you want an immutable `Box<Utf16Str>`.
///
/// # Panics
///
/// Panics on allocation failure or if a formatter returns `Err`.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "utf16")] {
/// use widestring::utf16str;
/// let arena = multitude::Arena::new();
/// let name = "Alice";
/// let s = multitude::strings::format_utf16!(in &arena, "Hello, {name}!");
/// assert_eq!(s.as_utf16_str(), utf16str!("Hello, Alice!"));
/// # }
/// ```
#[doc(hidden)]
#[macro_export]
#[cfg(feature = "utf16")]
macro_rules! __multitude_format_utf16 {
    (in $arena:expr, $($arg:tt)*) => {{
        let mut __multitude_buf = $crate::Arena::alloc_utf16_string($arena);
        ::core::fmt::Write::write_fmt(
            &mut __multitude_buf,
            ::core::format_args!($($arg)*),
        )
        .expect("a formatting trait implementation returned an error");
        __multitude_buf
    }};
}

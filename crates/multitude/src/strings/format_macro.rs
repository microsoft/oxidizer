// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// `format!`-style macro that writes into a fresh arena-backed
/// [`String`](crate::strings::String).
///
/// Freeze the result with [`String::into_arena_str`](crate::strings::String::into_arena_str)
/// if you want an immutable [`RcStr`](crate::strings::RcStr).
///
/// # Panics
///
/// Panics on allocation failure or if a formatter returns `Err`.
///
/// # Example
///
/// ```
/// let arena = multitude::Arena::new();
/// let name = "Alice";
/// let s = multitude::strings::format!(in &arena, "Hello, {name}!");
/// assert_eq!(&*s, "Hello, Alice!");
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __multitude_format {
    (in $arena:expr, $($arg:tt)*) => {{
        let mut __multitude_buf = $crate::Arena::alloc_string($arena);
        ::core::fmt::Write::write_fmt(
            &mut __multitude_buf,
            ::core::format_args!($($arg)*),
        )
        .expect("a formatting trait implementation returned an error");
        __multitude_buf
    }};
}

use core::fmt;

use allocator_api2::alloc::Allocator;

use crate::strings::String;

impl<A: Allocator + Clone> fmt::Write for String<'_, A> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        self.push(c);
        Ok(())
    }
}

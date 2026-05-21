// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Construct an arena-backed [`Vec`](crate::vec::Vec).
///
/// Supports the empty, list, and repeat forms of `vec!`.
///
/// # Panics
///
/// Panics on allocation failure.
///
/// # Examples
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let _v: multitude::vec::Vec<i32> = multitude::vec::vec![in &arena];
/// let v = multitude::vec::vec![in &arena; 1, 2, 3];
/// assert_eq!(&*v, &[1, 2, 3]);
/// let zeros = multitude::vec::vec![in &arena; 0_u32; 5];
/// assert_eq!(&*zeros, &[0, 0, 0, 0, 0]);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __multitude_vec {
    (in $arena:expr) => {
        $crate::vec::Vec::new_in($arena)
    };
    (in $arena:expr; $elem:expr; $n:expr) => {{
        let __multitude_n: ::core::primitive::usize = $n;
        let mut __multitude_buf = $crate::vec::Vec::with_capacity_in(__multitude_n, $arena);
        __multitude_buf.resize(__multitude_n, $elem);
        __multitude_buf
    }};
    (in $arena:expr; $($x:expr),+ $(,)?) => {{
        $crate::vec::Vec::from_iter_in([$($x),+], $arena)
    }};
}

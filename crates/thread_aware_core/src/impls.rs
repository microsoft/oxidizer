// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::num::{
    NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize, NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128,
    NonZeroUsize,
};
use core::time::Duration;
#[cfg(any(test, feature = "std"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "std"))]
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
#[cfg(any(test, feature = "std"))]
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "std"))]
use std::thread::ThreadId;

use crate::{NumaNode, Owner, Thread, ThreadAware};

/// Implements [`ThreadAware`] for a type that holds nothing bound to a thread, so a
/// move leaves it valid as-is and `relocate` has nothing to do.
macro_rules! impl_thread_aware {
    ($t:ty) => {
        impl ThreadAware for $t {
            #[inline]
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
    };
}

impl_thread_aware!(bool);
impl_thread_aware!(u8);
impl_thread_aware!(u16);
impl_thread_aware!(u32);
impl_thread_aware!(u64);
impl_thread_aware!(u128);
impl_thread_aware!(i8);
impl_thread_aware!(i16);
impl_thread_aware!(i32);
impl_thread_aware!(i64);
impl_thread_aware!(i128);
impl_thread_aware!(usize);
impl_thread_aware!(isize);
impl_thread_aware!(f32);
impl_thread_aware!(f64);
impl_thread_aware!(char);

impl_thread_aware!(NonZeroU8);
impl_thread_aware!(NonZeroU16);
impl_thread_aware!(NonZeroU32);
impl_thread_aware!(NonZeroU64);
impl_thread_aware!(NonZeroU128);
impl_thread_aware!(NonZeroUsize);
impl_thread_aware!(NonZeroI8);
impl_thread_aware!(NonZeroI16);
impl_thread_aware!(NonZeroI32);
impl_thread_aware!(NonZeroI64);
impl_thread_aware!(NonZeroI128);
impl_thread_aware!(NonZeroIsize);

impl_thread_aware!(String);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(PathBuf);
impl_thread_aware!(Duration);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(Path);
// Both socket-address variants contain only address, port, flow-info, and scope-id values;
// none of their fields carry thread-local or shared state that relocation could re-home.
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(SocketAddr);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(SocketAddrV4);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(SocketAddrV6);

impl_thread_aware!(str);
// Immutable process-lifetime string labels have no referent state to relocate and cannot dangle.
// This narrow reference exception does not extend to borrowed or mutable references in general.
impl_thread_aware!(&'static str);

impl_thread_aware!(Owner);
impl_thread_aware!(NumaNode);
#[cfg(any(test, feature = "std"))]
impl_thread_aware!(ThreadId);
impl_thread_aware!(Thread);

impl<T: ?Sized> ThreadAware for PhantomData<T>
where
    Self: Send,
{
    #[inline]
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

macro_rules! impl_thread_aware_tuple {
    ($head:ident, $($tail:ident,)*) => {
        impl<$head, $($tail),*> ThreadAware for ($head, $($tail),*)
            where
                $head: ThreadAware,
                $($tail: ThreadAware),*
                {
                    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
                        #[expect(non_snake_case, reason = "Macro-generated code uses uppercase identifiers for tuple elements")]
                        let ($head, $($tail),*) = self;
                        $head.relocate(source, destination);
                        $( $tail.relocate(source, destination); )*
                    }
                }

                // Recursively call the macro for the rest of the tuple
                impl_thread_aware_tuple!($($tail,)*);
    };

    () => {
        impl ThreadAware for () {
            #[inline]
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
    };
}

macro_rules! impl_thread_aware_fn {
    ($head:ident, $($tail:ident,)*) => {
        impl<R, $head, $($tail),*> ThreadAware for fn($head, $($tail),*) -> R {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }

        // Recursively call the macro for the rest of the function parameters
        impl_thread_aware_fn!($($tail,)*);
    };
    () => {
        impl<R> ThreadAware for fn() -> R {
            fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
        }
    }
}

macro_rules! impl_thread_aware_arities {
    ($($parameter:ident),* $(,)?) => {
        impl_thread_aware_tuple!($($parameter,)*);
        impl_thread_aware_fn!($($parameter,)*);
    };
}

// Match the established `thread_aware` boundary so compatibility code can rely on the same
// tuple and safe function-pointer arities.
impl_thread_aware_arities!(A, B, C, D, E, F, G, H, I, J, K, L);

impl<T, const N: usize> ThreadAware for [T; N]
where
    T: ThreadAware,
{
    #[inline]
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.as_mut_slice().relocate(source, destination);
    }
}

impl<T> ThreadAware for [T]
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Option<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        if let Some(value) = self {
            value.relocate(source, destination);
        }
    }
}

impl<T, E> ThreadAware for Result<T, E>
where
    T: ThreadAware,
    E: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        match self {
            Ok(value) => value.relocate(source, destination),
            Err(err) => err.relocate(source, destination),
        }
    }
}

impl<T> ThreadAware for Vec<T>
where
    T: ThreadAware,
{
    #[inline]
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.as_mut_slice().relocate(source, destination);
    }
}

impl<T> ThreadAware for VecDeque<T>
where
    T: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.iter_mut() {
            value.relocate(source, destination);
        }
    }
}

impl<T> ThreadAware for Box<T>
where
    T: ThreadAware + ?Sized,
{
    #[inline]
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        (**self).relocate(source, destination);
    }
}

impl<K, V> ThreadAware for BTreeMap<K, V>
where
    K: Send,
    V: ThreadAware,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}
/// Implements [`ThreadAware`] for a single-threaded interior-mutability wrapper.
///
/// `relocate` already holds `&mut self`, so the inner value is reachable through
/// `get_mut` without a borrow flag or a `Cell` round-trip.
macro_rules! impl_thread_aware_cell {
    ($t:ident) => {
        impl<T> ThreadAware for $t<T>
        where
            T: ThreadAware,
        {
            #[inline]
            fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
                self.get_mut().relocate(source, destination);
            }
        }
    };
}

impl_thread_aware_cell!(Cell);
impl_thread_aware_cell!(RefCell);

#[cfg(any(test, feature = "std"))]
impl<K, V, S> ThreadAware for HashMap<K, V, S>
where
    K: Send,
    V: ThreadAware,
    S: Send,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for value in self.values_mut() {
            value.relocate(source, destination);
        }
    }
}

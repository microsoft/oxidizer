// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-aware deserialization.
//!
//! Most users only derive [`DeserializeIn`](derive@DeserializeIn) and call an
//! [`Arena`] method. The trait method and Serde seed APIs are for custom
//! implementations.
//!
//! ```
//! # #[cfg(feature = "serde_json")]
//! # fn main() -> Result<(), serde_json::Error> {
//! use multitude::{Arena, Box};
//!
//! #[derive(multitude::de::DeserializeIn)]
//! struct Message {
//!     name: Box<str>,
//!     values: Box<[u64]>,
//! }
//!
//! let arena = Arena::new();
//! let message: Message = arena.deserialize_json(r#"{"name":"example","values":[1,2,3]}"#)?;
//! assert_eq!(message.name.as_str(), "example");
//! assert_eq!(&*message.values, &[1, 2, 3]);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "serde_json"))]
//! # fn main() {}
//! ```
//!
//! See the runnable
//! [typed graph](https://github.com/microsoft/oxidizer/blob/main/crates/multitude/examples/serde_arena_graph.rs),
//! [borrowing and reuse](https://github.com/microsoft/oxidizer/blob/main/crates/multitude/examples/serde_borrow_and_reuse.rs),
//! and [dynamic value and limits](https://github.com/microsoft/oxidizer/blob/main/crates/multitude/examples/serde_value_and_limits.rs)
//! examples for complete programs.
//!
//! # Ordinary Serde and `DeserializeIn`
//!
//! Serde's [`serde::Deserialize`] trait does not carry an allocator. Its
//! standard `String` and `Vec<T>` visitors therefore allocate through their
//! normal storage. [`DeserializeIn`] is the allocator-aware counterpart: it
//! receives an [`Arena`] and recursively passes that arena through
//! [`DeserializeInSeed`].
//!
//! The traits are independent and a type may derive both:
//!
//! | Situation | Recommended approach |
//! |---|---|
//! | Type you control with arena-backed fields | Derive `DeserializeIn` |
//! | Scalar fields | Supported automatically |
//! | Field implementing only ordinary `Deserialize` | Add `#[multitude(via_serde)]`; its allocations remain ordinary |
//! | Third-party root implementing only `Deserialize` | Deserialize it with ordinary Serde |
//! | Third-party type requiring arena-backed internals | Wrap it in a local newtype or write a custom `DeserializeIn` implementation |
//!
//! A blanket `DeserializeIn` implementation for every
//! `T: serde::Deserialize` would overlap arena-aware implementations and could
//! silently send allocations to the global allocator, so the conversion must
//! be explicit.
//!
//! ```
//! # #[cfg(feature = "serde_json")]
//! # fn main() -> Result<(), serde_json::Error> {
//! #[derive(serde::Deserialize)]
//! struct External {
//!     label: std::string::String,
//! }
//!
//! #[derive(serde::Deserialize, multitude::de::DeserializeIn)]
//! struct Envelope {
//!     id: u64,
//!     #[multitude(via_serde)]
//!     external: External,
//! }
//!
//! let arena = multitude::Arena::new();
//! let value: Envelope = arena.deserialize_json(r#"{"id":1,"external":{"label":"ordinary"}}"#)?;
//! assert_eq!(value.external.label, "ordinary");
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "serde_json"))]
//! # fn main() {}
//! ```
//!
//! # Choosing field storage
//!
//! - [`Box<str>`] and [`Box<[T]>`] are the usual immutable,
//!   uniquely owned field forms.
//! - [`Arc<str>`], [`Arc<[T]>`], [`Rc<str>`], and
//!   [`Rc<[T]>`] provide shared immutable storage.
//! - [`crate::Cow<'de, str>`] borrows decoded input only when the format can
//!   safely expose it for `'de`; otherwise it copies the decoded string into
//!   the arena. JSON strings containing escapes therefore use its owned
//!   variant.
//! - Standard `alloc::vec::Vec`, `BTreeMap`, and `BTreeSet` decode
//!   arena-aware elements, but their collection nodes or buffers still use
//!   their standard allocator. Use a frozen arena slice when the sequence
//!   buffer itself must be arena-owned.
//! - `#[multitude(via_serde)]` deliberately delegates one field to its
//!   ordinary [`serde::Deserialize`] implementation. This is useful for types
//!   that do not need arena storage, but their allocations are not redirected.
//!
//! # Root ownership and lifetime
//!
//! [`Arena::deserialize`] returns any [`DeserializeIn`] type. Choose
//! [`Box<T>`], [`Arc<T>`], or [`Rc<T>`] as the return type to allocate the root
//! in the corresponding arena smart pointer. These pointers keep their chunks
//! alive, so an entirely arena-owned graph can remain valid after the [`Arena`]
//! handle is dropped. Atomic roots require the value and allocator to be
//! `Send + Sync`.
//! [`Arena::deserialize_alloc`] instead returns an [`Alloc<'_, T>`] whose root
//! is local to the arena borrow. Nested fields retain the storage forms declared
//! by `T`.
//!
//! [`DeserializeInSeed`] integrates arena-aware values into another
//! Serde visitor. Only custom [`DeserializeIn`] implementations normally need
//! it.
//!
//! # Derive attributes
//!
//! The derive supports named, tuple, and unit structs; transparent structs;
//! and externally tagged enums. It follows Serde's deserialization behavior for
//! `rename`, `rename_all`, `alias`, `default`, `skip`, `deny_unknown_fields`,
//! `expecting`, explicit deserialize bounds, and `deserialize_with`/`with`.
//! Named structs and struct variants accept both map and ordered-sequence
//! representations.
//!
//! Arena-specific attributes are:
//!
//! - `#[multitude(via_serde)]` to use ordinary Serde for one field.
//! - `#[multitude(deserialize_with = "path")]` for a function with the
//!   signature `fn(&Arena<A>, D) -> Result<T, D::Error>`.
//! - `#[multitude(crate = "path")]` when `multitude` is renamed in
//!   `Cargo.toml`.
//!
//! `flatten`, internally or adjacently tagged enums, untagged enums, and
//! `remote` are rejected with compile-time diagnostics. Buffering those
//! representations cannot currently preserve the caller's `'de` borrowing
//! contract soundly, and `remote` would require a separate adapter API rather
//! than a foreign trait implementation.
//!
//! # Dynamic values and replay
//!
//! [`Value`] captures arbitrary Serde data in arena storage. Its map
//! representation preserves insertion order, duplicate keys, and non-string
//! keys. A borrowed `&Value` implements [`serde::Deserializer`], allowing data
//! to be buffered once and replayed into ordinary Serde types without parsing
//! the original format again.
//!
//! The dynamic model preserves the numeric category delivered by the source
//! deserializer. Formats that invoke opaque [`serde::de::EnumAccess`] cannot
//! expose enough payload shape for generic capture; externally tagged
//! string/map representations are replayable.
//!
//! # Reuse, limits, and failure behavior
//!
//! [`crate::strings::String::deserialize_reusing`] and
//! [`crate::vec::Vec::deserialize_reusing`] replace an existing growable arena
//! buffer while retaining reusable capacity. The destination is cleared first.
//! On an error it remains valid, but may contain the successfully decoded
//! prefix rather than its previous value.
//!
//! [`DeserializationLimits`] bounds nesting, sequence and map lengths, strings,
//! and byte strings. Use [`Arena::deserialize_with_limits`],
//! [`Arena::deserialize_alloc_with_limits`],
//! [`Arena::deserialize_json_with_limits`], or
//! [`Arena::deserialize_json_alloc_with_limits`]. Limits clamp untrusted size
//! hints before visitors reserve storage.
//!
//! Serde requires seeds to return the format's error type, so allocation and
//! limit failures are reported through [`serde::de::Error::custom`]. Failed
//! deserialization may consume arena capacity. General rollback would be
//! unsound because a custom implementation can let escape-capable smart
//! pointers outlive the failed operation.
//!
//! # JSON and `no_std`
//!
//! The base `serde` feature is format-independent and supports `no_std` with
//! `alloc`. The `serde_json` feature implies `serde` and adds
//! [`Arena::deserialize_json`], [`Arena::deserialize_json_alloc`], and their
//! resource-limited variants for strings and byte inputs. Those helpers require
//! one complete JSON value and reject trailing non-whitespace input. JSON may
//! use temporary parser scratch space while decoding escaped strings even
//! though the final arena-aware value is stored in the arena.

use core::fmt;
use core::marker::PhantomData;

use allocator_api2::alloc::Allocator;
use ptr_meta::Pointee;
use serde::de::{self, DeserializeSeed as _, Deserializer, Error as _, SeqAccess, Visitor};

use crate::{Alloc, Arc, Arena, Box, Cow, Rc};

mod containers;
mod deserialize_in;
mod deserialize_in_seed;
mod deserialize_seed;
#[cfg(feature = "serde_json")]
mod json;
mod limits;
mod reuse;
mod slice_visitor;
mod str_visitor;
mod value;

/// Arena-aware deserialization.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::DeserializeIn;
/// use serde::de::value::{Error, U64Deserializer};
///
/// # fn main() -> Result<(), Error> {
/// let arena = Arena::new();
/// let value = u64::deserialize_in(&arena, U64Deserializer::new(5))?;
/// assert_eq!(value, 5);
/// # Ok(())
/// # }
/// ```
pub use deserialize_in::DeserializeIn;
pub use deserialize_in_seed::DeserializeInSeed;
#[doc(hidden)]
pub use deserialize_seed::DeserializeSeed;
pub use limits::DeserializationLimits;
use limits::deserialize_seed_with_limits;
use slice_visitor::SliceVisitor;
use str_visitor::StrVisitor;
pub use value::{Entry, EnumValue, Map, Number, Value};

#[doc(hidden)]
pub mod __private {
    pub use allocator_api2;
    pub use serde;
}

/// Derive arena-aware deserialization for a type.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::DeserializeIn;
/// use serde::de::value::{Error, MapDeserializer};
///
/// #[derive(DeserializeIn)]
/// struct Flag {
///     enabled: bool,
/// }
///
/// # fn main() -> Result<(), Error> {
/// let arena = Arena::new();
/// let input = MapDeserializer::new([("enabled", true)].into_iter());
/// let flag = Flag::deserialize_in(&arena, input)?;
/// assert!(flag.enabled);
/// # Ok(())
/// # }
/// ```
pub use multitude_macros::DeserializeIn;

impl<'de, A: Allocator + Clone> DeserializeIn<'de, A> for Cow<'de, str, A> {
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CowVisitor<'a, A: Allocator + Clone>(&'a Arena<A>);

        impl<'de, A: Allocator + Clone> Visitor<'de> for CowVisitor<'_, A> {
            type Value = Cow<'de, str, A>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string")
            }

            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
                Ok(Cow::Borrowed(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                self.0.try_alloc_str_box(v).map(Cow::Owned).map_err(E::custom)
            }

            fn visit_string<E: de::Error>(self, v: alloc::string::String) -> Result<Self::Value, E> {
                self.visit_str(&v)
            }
        }

        deserializer.deserialize_str(CowVisitor(arena))
    }
}

impl<'de, A: Allocator + Clone> DeserializeIn<'de, A> for Cow<'de, [u8], A> {
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CowVisitor<'a, A: Allocator + Clone>(&'a Arena<A>);

        impl<'de, A: Allocator + Clone> Visitor<'de> for CowVisitor<'_, A> {
            type Value = Cow<'de, [u8], A>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a byte string")
            }

            fn visit_borrowed_bytes<E: de::Error>(self, v: &'de [u8]) -> Result<Self::Value, E> {
                Ok(Cow::Borrowed(v))
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                self.0.try_alloc_slice_copy_box(v).map(Cow::Owned).map_err(E::custom)
            }

            fn visit_byte_buf<E: de::Error>(self, v: alloc::vec::Vec<u8>) -> Result<Self::Value, E> {
                self.visit_bytes(&v)
            }
        }

        deserializer.deserialize_bytes(CowVisitor(arena))
    }
}

macro_rules! serialize_smart_pointer {
    ($pointer:ident) => {
        impl<T, A> serde::Serialize for crate::$pointer<T, A>
        where
            T: serde::Serialize + Pointee,
            A: Allocator + Clone,
        {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(&**self, serializer)
            }
        }

        impl<T, A> serde::Serialize for crate::$pointer<[T], A>
        where
            T: serde::Serialize,
            A: Allocator + Clone,
        {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(&**self, serializer)
            }
        }
    };
}

serialize_smart_pointer!(Arc);
serialize_smart_pointer!(Box);
serialize_smart_pointer!(Rc);

macro_rules! deserialize_via_serde {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl<'de, A: Allocator + Clone> DeserializeIn<'de, A> for $ty {
                #[inline]
                fn deserialize_in<D>(_arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    serde::Deserialize::deserialize(deserializer)
                }
            }
        )+
    };
}

deserialize_via_serde!(
    (),
    bool,
    char,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64
);

impl<'de, T, A> DeserializeIn<'de, A> for Option<T>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OptionVisitor<'a, T, A: Allocator + Clone> {
            arena: &'a Arena<A>,
            marker: PhantomData<fn() -> T>,
        }

        impl<'de, T, A> Visitor<'de> for OptionVisitor<'_, T, A>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = Option<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an optional value")
            }

            #[cfg_attr(test, mutants::skip)] // `Option<T>::default()` is also `None`
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            #[cfg_attr(test, mutants::skip)] // `Option<T>::default()` is also `None`
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                DeserializeInSeed::<T, A>::new(self.arena).deserialize(deserializer).map(Some)
            }
        }

        deserializer.deserialize_option(OptionVisitor {
            arena,
            marker: PhantomData,
        })
    }
}

impl<'de, A: Allocator + Clone> DeserializeIn<'de, A> for Box<str, A> {
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(StrVisitor::new(arena, |arena: &Arena<A>, value: &str| {
            arena.try_alloc_str_box(value)
        }))
    }
}

impl<'de, A> DeserializeIn<'de, A> for Arc<str, A>
where
    A: Allocator + Clone + Send + Sync,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(StrVisitor::new(arena, |arena: &Arena<A>, value: &str| {
            arena.try_alloc_str_arc(value)
        }))
    }
}

impl<'de, A: Allocator + Clone> DeserializeIn<'de, A> for Rc<str, A> {
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(StrVisitor::new(arena, |arena: &Arena<A>, value: &str| {
            arena.try_alloc_str_rc(value)
        }))
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Box<[T], A>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VisitorImpl<'a, T, A: Allocator + Clone>(SliceVisitor<'a, T, A>);

        impl<'de, T, A> Visitor<'de> for VisitorImpl<'_, T, A>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = Box<[T], A>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S>(self, seq: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                self.0.deserialize_vec(seq)?.try_into_boxed_slice().map_err(S::Error::custom)
            }
        }

        deserializer.deserialize_seq(VisitorImpl(SliceVisitor::new(arena)))
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Arc<[T], A>
where
    T: DeserializeIn<'de, A> + Send + Sync,
    A: Allocator + Clone + Send + Sync,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VisitorImpl<'a, T, A: Allocator + Clone>(SliceVisitor<'a, T, A>);

        impl<'de, T, A> Visitor<'de> for VisitorImpl<'_, T, A>
        where
            T: DeserializeIn<'de, A> + Send + Sync,
            A: Allocator + Clone + Send + Sync,
        {
            type Value = Arc<[T], A>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S>(self, seq: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                self.0.deserialize_vec(seq)?.try_into_arc_slice().map_err(S::Error::custom)
            }
        }

        deserializer.deserialize_seq(VisitorImpl(SliceVisitor::new(arena)))
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Rc<[T], A>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VisitorImpl<'a, T, A: Allocator + Clone>(SliceVisitor<'a, T, A>);

        impl<'de, T, A> Visitor<'de> for VisitorImpl<'_, T, A>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = Rc<[T], A>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S>(self, seq: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                self.0.deserialize_vec(seq)?.try_into_rc_slice().map_err(S::Error::custom)
            }
        }

        deserializer.deserialize_seq(VisitorImpl(SliceVisitor::new(arena)))
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Box<T, A>
where
    T: DeserializeIn<'de, A> + Pointee,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = T::deserialize_in(arena, deserializer)?;
        arena.try_alloc_box(value).map_err(D::Error::custom)
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Arc<T, A>
where
    T: DeserializeIn<'de, A> + Pointee + Send + Sync,
    A: Allocator + Clone + Send + Sync,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = T::deserialize_in(arena, deserializer)?;
        arena.try_alloc_arc(value).map_err(D::Error::custom)
    }
}

impl<'de, T, A> DeserializeIn<'de, A> for Rc<T, A>
where
    T: DeserializeIn<'de, A> + Pointee,
    A: Allocator + Clone,
{
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = T::deserialize_in(arena, deserializer)?;
        arena.try_alloc_rc(value).map_err(D::Error::custom)
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Deserialize an arena-aware value.
    ///
    /// The return type selects the complete output type, including root
    /// ownership. In particular, [`Box<T>`], [`Arc<T>`], and [`Rc<T>`] use
    /// their [`DeserializeIn`] implementations to allocate the root in the
    /// arena.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input or allocation
    /// failure.
    ///
    /// ```
    /// use multitude::Arena;
    /// use serde::de::value::{Error, U64Deserializer};
    ///
    /// # fn main() -> Result<(), Error> {
    /// let arena = Arena::new();
    /// use multitude::{Arc, Box, Rc};
    ///
    /// let boxed: Box<u64> = arena.deserialize(U64Deserializer::new(10))?;
    /// let shared: Arc<u64> = arena.deserialize(U64Deserializer::new(11))?;
    /// let local: Rc<u64> = arena.deserialize(U64Deserializer::new(12))?;
    /// assert_eq!((*boxed, *shared, *local), (10, 11, 12));
    /// # Ok(())
    /// # }
    /// ```
    pub fn deserialize<'de, T, D>(&self, deserializer: D) -> Result<T, D::Error>
    where
        T: DeserializeIn<'de, A>,
        D: Deserializer<'de>,
    {
        T::deserialize_in(self, deserializer)
    }

    /// Deserialize a value and store its root in an arena-local [`Alloc`].
    ///
    /// Unlike arena smart pointers, the returned handle borrows this arena and
    /// cannot outlive it. Nested fields retain the storage forms declared by
    /// `T`; this method changes only root ownership.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input or allocation
    /// failure.
    pub fn deserialize_alloc<'arena, 'de, T, D>(&'arena self, deserializer: D) -> Result<Alloc<'arena, T>, D::Error>
    where
        T: DeserializeIn<'de, A>,
        D: Deserializer<'de>,
    {
        let value = T::deserialize_in(self, deserializer)?;
        self.try_alloc(value).map_err(D::Error::custom)
    }

    /// Deserialize an arena-aware value while enforcing resource limits.
    ///
    /// As with [`Arena::deserialize`], the return type selects root ownership.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input, allocation
    /// failure, or a limit violation.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::DeserializationLimits;
    /// use serde::de::value::{Error, U64Deserializer};
    ///
    /// # fn main() -> Result<(), Error> {
    /// let arena = Arena::new();
    /// use multitude::Box;
    ///
    /// let value: Box<u64> = arena
    ///     .deserialize_with_limits(U64Deserializer::new(11), DeserializationLimits::unlimited())?;
    /// assert_eq!(*value, 11);
    /// # Ok(())
    /// # }
    /// ```
    pub fn deserialize_with_limits<'de, T, D>(&self, deserializer: D, limits: DeserializationLimits) -> Result<T, D::Error>
    where
        T: DeserializeIn<'de, A>,
        D: Deserializer<'de>,
    {
        deserialize_seed_with_limits(deserializer, DeserializeInSeed::<T, A>::new(self), limits)
    }

    /// Deserialize a value with resource limits and store its root in an
    /// arena-local [`Alloc`].
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input, allocation
    /// failure, or a limit violation.
    pub fn deserialize_alloc_with_limits<'arena, 'de, T, D>(
        &'arena self,
        deserializer: D,
        limits: DeserializationLimits,
    ) -> Result<Alloc<'arena, T>, D::Error>
    where
        T: DeserializeIn<'de, A>,
        D: Deserializer<'de>,
    {
        let value = self.deserialize_with_limits(deserializer, limits)?;
        self.try_alloc(value).map_err(D::Error::custom)
    }
}

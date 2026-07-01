// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`RouteMatch`] resolved generated-router match type.

use crate::binding::Binding;

/// The number of path-variable bindings [`RouteMatch`] stores inline before
/// spilling to the heap. Real REST templates bind only a handful of path
/// variables, so the common case never allocates.
const INLINE_BINDINGS: usize = 3;

/// Zero-allocation-for-the-common-case storage for a match's bindings.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Bindings<'p> {
    /// Up to [`INLINE_BINDINGS`] bindings stored inline (no heap allocation).
    Inline { buf: [Binding<'p>; INLINE_BINDINGS], len: usize },
    /// Overflow for the (unusual) case of more than [`INLINE_BINDINGS`] bindings.
    Heap(Vec<Binding<'p>>),
}

impl<'p> Bindings<'p> {
    fn from_slice(bindings: &[Binding<'p>]) -> Self {
        if bindings.len() <= INLINE_BINDINGS {
            let mut buf: [Binding<'p>; INLINE_BINDINGS] = [Binding::EMPTY; INLINE_BINDINGS];
            buf[..bindings.len()].copy_from_slice(bindings);
            Self::Inline { buf, len: bindings.len() }
        } else {
            Self::Heap(bindings.to_vec())
        }
    }

    fn as_slice(&self) -> &[Binding<'p>] {
        match self {
            Self::Inline { buf, len } => &buf[..*len],
            Self::Heap(vec) => vec,
        }
    }
}

/// The result of a successful route resolution by a generated router: the
/// resolved RPC name and the path-variable bindings captured for it.
///
/// Bindings are stored inline for the common small-count case, so resolving a
/// route performs no heap allocation.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::{Binding, RouteMatch};
///
/// let route = RouteMatch::new("Library.GetShelf", vec![Binding::new(&["shelf"], "7")]);
/// assert_eq!(route.rpc(), "Library.GetShelf");
/// assert_eq!(route.bindings().len(), 1);
/// assert_eq!(route.get(&["shelf"]), Some("7"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteMatch<'p> {
    rpc: &'static str,
    bindings: Bindings<'p>,
}

impl<'p> RouteMatch<'p> {
    /// Creates a route match for `rpc` with the given captured `bindings`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Binding, RouteMatch};
    ///
    /// let route = RouteMatch::new("Library.GetBook", vec![Binding::new(&["book"], "rust")]);
    /// assert_eq!(route.rpc(), "Library.GetBook");
    /// assert_eq!(route.get(&["book"]), Some("rust"));
    /// ```
    #[must_use]
    pub fn new(rpc: &'static str, bindings: Vec<Binding<'p>>) -> Self {
        let bindings = if bindings.len() <= INLINE_BINDINGS {
            Bindings::from_slice(&bindings)
        } else {
            Bindings::Heap(bindings)
        };
        Self { rpc, bindings }
    }

    /// Creates a route match for `rpc`, copying `bindings` into inline storage
    /// (no heap allocation for the usual small counts).
    ///
    /// This is the zero-allocation constructor generated routers call.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Binding, RouteMatch};
    ///
    /// let bindings = [
    ///     Binding::new(&["shelf"], "7"),
    ///     Binding::new(&["book"], "rust"),
    /// ];
    /// let route = RouteMatch::with_bindings("Library.GetBook", &bindings);
    /// assert_eq!(route.bindings(), bindings);
    /// assert_eq!(route.get(&["book"]), Some("rust"));
    /// ```
    #[must_use]
    pub fn with_bindings(rpc: &'static str, bindings: &[Binding<'p>]) -> Self {
        Self {
            rpc,
            bindings: Bindings::from_slice(bindings),
        }
    }

    /// The resolved RPC name (the gRPC method the request transcodes to).
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::RouteMatch;
    ///
    /// let route = RouteMatch::new("Library.ListShelves", Vec::new());
    /// assert_eq!(route.rpc(), "Library.ListShelves");
    /// ```
    #[must_use]
    pub const fn rpc(&self) -> &'static str {
        self.rpc
    }

    /// The captured path-variable bindings, in template declaration order.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Binding, RouteMatch};
    ///
    /// let route = RouteMatch::with_bindings("Library.GetShelf", &[Binding::new(&["shelf"], "7")]);
    /// assert_eq!(route.bindings()[0].value(), "7");
    /// ```
    #[must_use]
    pub fn bindings(&self) -> &[Binding<'p>] {
        self.bindings.as_slice()
    }

    /// Looks up a captured binding by its dotted field path.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Binding, RouteMatch};
    ///
    /// let route = RouteMatch::with_bindings("Library.GetShelf", &[Binding::new(&["shelf"], "7")]);
    /// assert_eq!(route.get(&["shelf"]), Some("7"));
    /// assert_eq!(route.get(&["book"]), None);
    /// ```
    #[must_use]
    pub fn get(&self, field_path: &[&str]) -> Option<&'p str> {
        self.bindings().iter().find(|b| b.field_path() == field_path).map(Binding::value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_match_lookup() {
        let m = RouteMatch::new("GetShelf", vec![Binding::new(&["shelf", "id"], "7")]);
        assert_eq!(m.rpc(), "GetShelf");
        assert_eq!(m.get(&["shelf", "id"]), Some("7"));
        assert_eq!(m.get(&["nope"]), None);
        assert_eq!(m.bindings().len(), 1);
    }

    #[test]
    fn new_spills_to_heap_past_inline_capacity() {
        let bindings: Vec<Binding<'static>> = (0..INLINE_BINDINGS + 2).map(|_| Binding::new(&["x"], "v")).collect();
        let m = RouteMatch::new("Big", bindings);
        assert!(matches!(m.bindings, Bindings::Heap(_)));
        assert_eq!(m.bindings().len(), INLINE_BINDINGS + 2);
    }

    #[test]
    fn with_bindings_stays_inline_for_small_counts() {
        let m = RouteMatch::with_bindings("Get", &[Binding::new(&["a"], "1"), Binding::new(&["b"], "2")]);
        assert!(matches!(m.bindings, Bindings::Inline { .. }));
        assert_eq!(m.bindings().len(), 2);
        assert_eq!(m.get(&["b"]), Some("2"));
    }

    #[test]
    fn with_bindings_spills_to_heap_past_inline_capacity() {
        let bindings: Vec<Binding<'static>> = (0..INLINE_BINDINGS + 3).map(|_| Binding::new(&["x"], "v")).collect();
        let m = RouteMatch::with_bindings("Big", &bindings);
        assert!(matches!(m.bindings, Bindings::Heap(_)));
        assert_eq!(m.bindings().len(), INLINE_BINDINGS + 3);
    }
}

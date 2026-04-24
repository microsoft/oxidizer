//! Borrowed cons-list of `TypeId`s representing the in-flight resolution chain.
//!
//! `PathStack` flows downward through the resolver's `resolve` calls so that
//! later phases can perform path-aware lookups against the
//! [`PathCache`](crate::path_cache::PathCache). Each `push` allocates a fresh
//! borrowed cell on the call stack — no heap allocation is required.
//!
//! In phase 2, the path is plumbed through but no caller registers an override
//! that would create a multi-element cache key, so the path content is not
//! consulted by the cache. The plumbing exists so phase 3+ can light up
//! suffix-matching without further structural changes.

use std::any::TypeId;

/// A borrowed view of the in-flight resolution chain.
///
/// The chain is built as a singly-linked list of borrowed cells. The root
/// (oldest) link is at one end; the most recently pushed link is at the
/// current cell.
#[derive(Debug, Clone, Copy)]
pub struct PathStack<'a> {
    inner: PathStackInner<'a>,
}

#[derive(Debug, Clone, Copy)]
enum PathStackInner<'a> {
    Empty,
    Cons {
        parent: &'a PathStack<'a>,
        head: TypeId,
        len: usize,
    },
}

impl<'a> PathStack<'a> {
    /// Returns an empty path stack representing the root of resolution.
    #[must_use]
    pub fn root() -> Self {
        Self {
            inner: PathStackInner::Empty,
        }
    }

    /// Returns the number of elements currently in the stack.
    #[must_use]
    pub fn len(&self) -> usize {
        match self.inner {
            PathStackInner::Empty => 0,
            PathStackInner::Cons { len, .. } => len,
        }
    }

    /// Returns whether the stack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self.inner, PathStackInner::Empty)
    }

    /// Returns a new path stack that extends `self` by appending `head`.
    ///
    /// The returned stack borrows from `self`, so its lifetime is bounded by
    /// the caller's stack frame. The borrow is `Copy`-able for ergonomic
    /// re-passing into recursive resolution.
    #[must_use]
    pub fn push(&'a self, head: TypeId) -> Self {
        let len = self.len() + 1;
        Self {
            inner: PathStackInner::Cons { parent: self, head, len },
        }
    }

    /// Materializes the path as a `Vec<TypeId>` in root-first order.
    ///
    /// Allocates. Used for constructing cache keys; later phases will avoid
    /// the allocation on hot paths via in-place suffix matching.
    #[must_use]
    #[expect(
        clippy::wrong_self_convention,
        reason = "PathStack is borrow-only; `to_vec` mirrors slice/Vec semantics and takes &self"
    )]
    pub fn to_vec(&self) -> Vec<TypeId> {
        let mut buf = Vec::with_capacity(self.len());
        self.collect_into(&mut buf);
        buf
    }

    fn collect_into(&self, buf: &mut Vec<TypeId>) {
        if let PathStackInner::Cons { parent, head, .. } = &self.inner {
            parent.collect_into(buf);
            buf.push(*head);
        }
    }
}

impl Default for PathStack<'_> {
    fn default() -> Self {
        Self::root()
    }
}

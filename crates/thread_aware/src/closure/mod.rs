// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers for defining and calling [`ThreadAware`] closures.

mod erased;

use std::pin::Pin;

pub(crate) use erased::ErasedClosureOnce;

use crate::ThreadAware;
use crate::affinity::Affinity;

/// A boxed, pinned, `Send` future - the return type of async closure calls.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Marks `FnOnce()`-like closures whose captured values all implement [`ThreadAware`].
///
/// Use [`closure_once`] function to construct these.
pub trait ThreadAwareFnOnce<T: ?Sized>: ThreadAware {
    /// Calls the closure, consuming it in the process.
    fn call_once(self) -> T;
}

/// Marks `Fn()`-like closure whose captured values all implement [`ThreadAware`].
///
/// This trait is used to define closures that can be called multiple times, without consuming the closure.
pub trait ThreadAwareFn<T>: ThreadAware {
    /// Calls the closure, returning the result.
    fn call(&self) -> T;
}

/// Marks `FnMut()`-like closure whose captured values all implement [`ThreadAware`].
///
/// This trait is used to define closures that can be called mutably, allowing the closure to modify its internal state.
pub trait ThreadAwareFnMut<T>: ThreadAware {
    /// Calls the closure mutably, returning the result.
    fn call_mut(&mut self) -> T;
}

/// Async equivalent of [`ThreadAwareFnOnce`] - calls the closure once, returning a [`BoxFuture`].
///
/// Use [`async_closure_once`] to construct an implementation.
pub trait ThreadAwareAsyncFnOnce<T>: ThreadAware {
    /// Calls the async closure, consuming it.
    fn call_once(self: Box<Self>) -> BoxFuture<'static, T>;
}

/// Async equivalent of [`ThreadAwareFn`] - calls the closure by shared reference, returning a [`BoxFuture`].
///
/// Use [`async_closure`] to construct an implementation.
pub trait ThreadAwareAsyncFn<T>: ThreadAware {
    /// Calls the async closure by shared reference.
    fn call(&self) -> BoxFuture<'_, T>;
}

/// Async equivalent of [`ThreadAwareFnMut`] - calls the closure by mutable reference, returning a [`BoxFuture`].
///
/// Use [`async_closure_mut`] to construct an implementation.
pub trait ThreadAwareAsyncFnMut<T>: ThreadAware {
    /// Calls the async closure by mutable reference.
    fn call_mut(&mut self) -> BoxFuture<'_, T>;
}

/// A common implementation of [`ThreadAwareFn`].
///
/// Construct this using the [`closure`] function.
#[derive(Debug, Copy, Hash)]
pub struct Closure<T, D> {
    data: D,
    f: fn(&D) -> T,
}

impl<T, D> Clone for Closure<T, D>
where
    D: Clone,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D> ThreadAwareFn<T> for Closure<T, D>
where
    D: ThreadAware,
{
    fn call(&self) -> T {
        (self.f)(&self.data)
    }
}

impl<T, D> ThreadAwareFnMut<T> for Closure<T, D>
where
    D: ThreadAware,
{
    fn call_mut(&mut self) -> T {
        self.call()
    }
}

impl<T, D> ThreadAwareFnOnce<T> for Closure<T, D>
where
    D: ThreadAware,
{
    fn call_once(self) -> T {
        self.call()
    }
}

impl<T, D> ThreadAware for Closure<T, D>
where
    D: ThreadAware,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.data.relocate(source, destination);
    }
}

/// A common implementation of [`ThreadAwareFnOnce`].
///
/// Construct this using the [`closure_once`] function.
#[derive(Debug, Copy, Hash)]
pub struct ClosureOnce<T, D> {
    data: D,
    f: fn(D) -> T,
}

impl<T, D> Clone for ClosureOnce<T, D>
where
    D: Clone,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D> ThreadAwareFnOnce<T> for ClosureOnce<T, D>
where
    D: ThreadAware,
{
    fn call_once(self) -> T {
        (self.f)(self.data)
    }
}

impl<T, D> ThreadAware for ClosureOnce<T, D>
where
    D: ThreadAware,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.data.relocate(source, destination);
    }
}

/// A common implementation of [`ThreadAwareFnMut`].
///
/// Construct this using the [`closure_mut`] function.
#[derive(Debug, Copy, Hash)]
pub struct ClosureMut<T, D> {
    data: D,
    f: fn(&mut D) -> T,
}

impl<T, D> Clone for ClosureMut<T, D>
where
    D: Clone,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D> ThreadAwareFnMut<T> for ClosureMut<T, D>
where
    D: ThreadAware,
{
    fn call_mut(&mut self) -> T {
        (self.f)(&mut self.data)
    }
}

impl<T, D> ThreadAwareFnOnce<T> for ClosureMut<T, D>
where
    D: ThreadAware,
{
    fn call_once(mut self) -> T {
        self.call_mut()
    }
}

impl<T, D> ThreadAware for ClosureMut<T, D>
where
    D: ThreadAware,
{
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.data.relocate(source, destination);
    }
}

/// Constructs a [`Closure`].
///
/// Create a closure-like object by explicitly providing closed-over
/// value and a function pointer to operate on that value, essentially simulating a
/// parameterless closure that ensures that captured data implements [`ThreadAware`].
pub fn closure<T, D>(data: D, f: fn(&D) -> T) -> Closure<T, D>
where
    D: ThreadAware,
{
    Closure { data, f }
}

/// Constructs a [`ClosureMut`].
///
/// Create a closure-like object by explicitly providing closed-over
/// value and a function pointer to operate on that value, essentially simulating a
/// parameterless closure that ensures that captured data implements [`ThreadAware`].
pub fn closure_mut<T, D>(data: D, f: fn(&mut D) -> T) -> ClosureMut<T, D>
where
    D: ThreadAware,
{
    ClosureMut { data, f }
}

/// Constructs a [`ClosureOnce`].
///
/// Create a closure-like object by explicitly providing closed-over
/// value and a function pointer to operate on that value, essentially simulating a
/// parameterless closure that ensures that captured data implements [`ThreadAware`].
///
/// Usage:
/// ```rust
/// # use thread_aware::{ThreadAware, closure::closure_once, closure::ThreadAwareFnOnce};
/// # use thread_aware::affinity::*;
/// struct Transferable;
/// impl ThreadAware for Transferable {
///     // ...
///     # fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {}
/// }
///
/// let closure = closure_once(Transferable, |transferable| {
///     // do stuff with transferable
/// });
///
/// closure.call_once();
///
/// let closure_with_multiple_captured = closure_once((Transferable, Transferable), |(a, b)| {
///     // do stuff with a and b
/// });
///
/// closure_with_multiple_captured.call_once();
/// ```
///
/// This exists because Rust closures don't give us control over the types of captured values.
pub fn closure_once<T, D>(data: D, f: fn(D) -> T) -> ClosureOnce<T, D>
where
    D: ThreadAware,
{
    ClosureOnce { data, f }
}

// --- Async closure types ---

/// Async equivalent of [`Closure`] - can be called multiple times by shared reference.
///
/// The function pointer receives `&D` and must return a [`BoxFuture`].
/// Construct this using the [`async_closure`] function.
#[derive(Copy)]
pub struct AsyncClosure<T, D> {
    data: D,
    f: for<'a> fn(&'a D) -> BoxFuture<'a, T>,
}

impl<T, D: std::fmt::Debug> std::fmt::Debug for AsyncClosure<T, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncClosure").field("data", &self.data).finish_non_exhaustive()
    }
}

impl<T, D: Clone> Clone for AsyncClosure<T, D> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D: ThreadAware> AsyncClosure<T, D> {
    /// Calls the async closure by shared reference.
    pub fn call(&self) -> BoxFuture<'_, T> {
        (self.f)(&self.data)
    }
}

impl<T, D: ThreadAware> ThreadAwareAsyncFn<T> for AsyncClosure<T, D> {
    fn call(&self) -> BoxFuture<'_, T> {
        self.call()
    }
}

impl<T, D: ThreadAware> ThreadAwareAsyncFnMut<T> for AsyncClosure<T, D> {
    fn call_mut(&mut self) -> BoxFuture<'_, T> {
        self.call()
    }
}

impl<T, D: ThreadAware + 'static> ThreadAwareAsyncFnOnce<T> for AsyncClosure<T, D>
where
    T: 'static,
{
    fn call_once(self: Box<Self>) -> BoxFuture<'static, T> {
        Box::pin(async move { self.call().await })
    }
}

impl<T, D: ThreadAware> ThreadAware for AsyncClosure<T, D> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.data.relocate(source, destination);
    }
}

/// Async equivalent of [`ClosureOnce`] - can be called exactly once, consuming `self`.
///
/// The function pointer receives owned `D` and must return a [`BoxFuture`].
/// Construct this using the [`async_closure_once`] function.
#[derive(Copy)]
pub struct AsyncClosureOnce<T, D> {
    data: D,
    f: fn(D) -> BoxFuture<'static, T>,
}

impl<T, D: std::fmt::Debug> std::fmt::Debug for AsyncClosureOnce<T, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncClosureOnce").field("data", &self.data).finish_non_exhaustive()
    }
}

impl<T, D: Clone> Clone for AsyncClosureOnce<T, D> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D: ThreadAware> AsyncClosureOnce<T, D> {
    /// Calls the async closure, consuming it.
    pub fn call_once(self) -> BoxFuture<'static, T> {
        (self.f)(self.data)
    }
}

impl<T, D: ThreadAware> ThreadAwareAsyncFnOnce<T> for AsyncClosureOnce<T, D> {
    fn call_once(self: Box<Self>) -> BoxFuture<'static, T> {
        (self.f)(self.data)
    }
}

impl<T, D: ThreadAware> ThreadAware for AsyncClosureOnce<T, D> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.data.relocate(source, destination);
    }
}

/// Async equivalent of [`ClosureMut`] - can be called multiple times by mutable reference.
///
/// The function pointer receives `&mut D` and must return a [`BoxFuture`].
/// Construct this using the [`async_closure_mut`] function.
#[derive(Copy)]
pub struct AsyncClosureMut<T, D> {
    data: D,
    f: for<'a> fn(&'a mut D) -> BoxFuture<'a, T>,
}

impl<T, D: std::fmt::Debug> std::fmt::Debug for AsyncClosureMut<T, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncClosureMut").field("data", &self.data).finish_non_exhaustive()
    }
}

impl<T, D: Clone> Clone for AsyncClosureMut<T, D> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D: ThreadAware> AsyncClosureMut<T, D> {
    /// Calls the async closure by mutable reference.
    pub fn call_mut(&mut self) -> BoxFuture<'_, T> {
        (self.f)(&mut self.data)
    }
}

impl<T, D: ThreadAware> ThreadAwareAsyncFnMut<T> for AsyncClosureMut<T, D> {
    fn call_mut(&mut self) -> BoxFuture<'_, T> {
        self.call_mut()
    }
}

impl<T, D: ThreadAware + 'static> ThreadAwareAsyncFnOnce<T> for AsyncClosureMut<T, D>
where
    T: 'static,
{
    fn call_once(mut self: Box<Self>) -> BoxFuture<'static, T> {
        Box::pin(async move { self.call_mut().await })
    }
}

impl<T, D: ThreadAware> ThreadAware for AsyncClosureMut<T, D> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.data.relocate(source, destination);
    }
}

/// Constructs an [`AsyncClosure`] - the async equivalent of [`closure`].
///
/// The function pointer receives `&D` and must return a [`BoxFuture`].
/// Use `Box::pin(async move { ... })` in the function body.
///
/// # Examples
///
/// ```rust
/// use thread_aware::closure::async_closure;
///
/// fn my_async_fn(x: &i32) -> thread_aware::closure::BoxFuture<'_, i32> {
///     Box::pin(async move { *x + 1 })
/// }
///
/// let c = async_closure(42, my_async_fn);
/// ```
pub fn async_closure<T, D>(data: D, f: for<'a> fn(&'a D) -> BoxFuture<'a, T>) -> AsyncClosure<T, D>
where
    D: ThreadAware,
{
    AsyncClosure { data, f }
}

/// Constructs an [`AsyncClosureMut`] - the async equivalent of [`closure_mut`].
///
/// The function pointer receives `&mut D` and must return a [`BoxFuture`].
/// Use `Box::pin(async move { ... })` in the function body.
pub fn async_closure_mut<T, D>(data: D, f: for<'a> fn(&'a mut D) -> BoxFuture<'a, T>) -> AsyncClosureMut<T, D>
where
    D: ThreadAware,
{
    AsyncClosureMut { data, f }
}

/// Constructs an [`AsyncClosureOnce`] - the async equivalent of [`closure_once`].
///
/// The function pointer receives owned `D` and must return a [`BoxFuture`].
/// Use `Box::pin(async move { ... })` in the function body.
pub fn async_closure_once<T, D>(data: D, f: fn(D) -> BoxFuture<'static, T>) -> AsyncClosureOnce<T, D>
where
    D: ThreadAware,
{
    AsyncClosureOnce { data, f }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affinity::pinned_affinities;

    #[test]
    fn async_closure_once_compiles() {
        let c = async_closure_once(42, |x| Box::pin(async move { x + 1 }));
        let fut = c.call_once();
        let result = futures::executor::block_on(fut);
        assert_eq!(result, 43);
    }

    #[test]
    fn async_closure_compiles() {
        let c = async_closure(42, |x| Box::pin(async move { *x + 1 }));
        let result = futures::executor::block_on(c.call());
        assert_eq!(result, 43);
    }

    #[test]
    fn async_closure_mut_compiles() {
        fn increment(x: &mut i32) -> BoxFuture<'_, i32> {
            *x += 1;
            let val = *x;
            Box::pin(async move { val })
        }

        let mut c = async_closure_mut(0_i32, increment);
        let r1 = futures::executor::block_on(c.call_mut());
        let r2 = futures::executor::block_on(c.call_mut());
        assert_eq!(r1, 1);
        assert_eq!(r2, 2);
    }

    #[test]
    fn boxed_once() {
        let x = closure_once(42, |x| x + 1);
        let y = Box::new(x);
        let _z = y.call_once();
    }

    #[test]
    fn more_stuff() {
        let x = Closure { data: 42, f: |x| x + 1 };

        let _y = x.call();
    }

    #[test]
    fn something() {
        fn takes_mut(mut x: impl ThreadAwareFnMut<i32>) {
            let _y = x.call_mut();
        }

        let x = closure(42, |x| x + 1);
        takes_mut(x);
    }

    #[expect(clippy::empty_structs_with_brackets, reason = " Testing non-clone behavior")]
    #[test]
    fn non_clone() {
        struct MyStruct {}

        let y = closure((), |()| MyStruct {});
        let _z = y.call();
    }

    #[expect(clippy::redundant_clone, reason = "Testing clone behavior")]
    #[test]
    fn can_clone() {
        let x = Closure {
            data: String::from("Hello, world!"),
            f: |_| 43,
        };

        assert_eq!(x.call(), 43);
        let y = x.clone();
        assert_eq!(y.call(), 43);
    }

    // Tests for Closure<T, D>

    #[test]
    fn test_closure_clone() {
        // Test with Vec
        let c = closure(vec![1, 2, 3], std::vec::Vec::len);
        let cloned = c.clone();
        assert_eq!(c.call(), 3);
        assert_eq!(cloned.call(), 3);

        // Test with String
        let c = Closure {
            data: String::from("test"),
            f: |s| s.len(),
        };

        let cloned = c.clone();
        assert_eq!(c.call(), 4);
        assert_eq!(cloned.call(), 4);
    }

    #[test]
    fn test_closure_thread_aware() {
        let affinities = pinned_affinities(&[2, 2]);

        // Test with i32
        let mut c = closure(42_i32, |x| x + 1);
        c.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(c.call(), 43);

        // Test with Vec
        let mut c = closure(vec![10, 20, 30], |v| v.iter().sum::<i32>());
        c.relocate(Some(affinities[0]), affinities[2]);
        assert_eq!(c.call(), 60);

        // Test with same affinity (String)
        let mut c = closure(String::from("hello"), |s| s.to_uppercase());
        c.relocate(Some(affinities[0]), affinities[0]);
        assert_eq!(c.call(), "HELLO");
    }

    #[test]
    fn test_closure_thread_aware_fn_mut() {
        // Test with i32 - multiple calls should give same result
        let mut c = closure(5_i32, |x| x * 2);
        assert_eq!(c.call_mut(), 10);
        assert_eq!(c.call_mut(), 10);

        // Test with Vec - multiple calls should give same result
        let mut c = closure(vec![1, 2, 3, 4], std::vec::Vec::len);

        assert_eq!(c.call_mut(), 4);
        assert_eq!(c.call_mut(), 4);
        assert_eq!(c.call_mut(), 4);
    }

    // Tests for ClosureOnce<T, D>

    #[test]
    fn test_closure_once_clone() {
        // Test with i32
        let closure = closure_once(100_i32, |x| x * 2);
        let cloned = closure;
        assert_eq!(closure.call_once(), 200);
        assert_eq!(cloned.call_once(), 200);

        // Test with complex data (tuple)
        let closure = ClosureOnce {
            data: (String::from("test"), vec![1, 2, 3]),
            f: |(s, v)| format!("{}: {}", s, v.len()),
        };

        let cloned = closure.clone();
        assert_eq!(closure.call_once(), "test: 3");
        assert_eq!(cloned.call_once(), "test: 3");
    }

    #[test]
    fn test_closure_once_thread_aware() {
        let affinities = pinned_affinities(&[2, 3]);

        // Test with String
        let mut closure = closure_once(String::from("world"), |s| format!("Hello, {s}!"));
        closure.relocate(Some(affinities[0]), affinities[1]);
        assert_eq!(closure.call_once(), "Hello, world!");

        // Test with complex data (tuple of Vecs)
        let data = (vec![1, 2, 3], vec![4, 5, 6]);
        let mut closure = closure_once(data, |(a, b)| a.len() + b.len());
        closure.relocate(Some(affinities[1]), affinities[3]);
        assert_eq!(closure.call_once(), 6);

        // Test cross-NUMA transfer
        let mut closure = closure_once(42_i32, |x| x + 100);
        closure.relocate(Some(affinities[0]), affinities[2]);
        assert_eq!(closure.call_once(), 142);
    }

    // Tests for ClosureMut<T, D>

    #[test]
    fn test_closure_mut_clone() {
        // Test with i32 - clone creates independent copies
        let closure = closure_mut(10_i32, |x| {
            *x += 5;
            *x
        });
        let cloned = closure;
        let mut c1 = closure;
        let mut c2 = cloned;
        assert_eq!(c1.call_mut(), 15);
        assert_eq!(c2.call_mut(), 15);

        // Test with Vec - independent copies continue to grow independently
        let closure = ClosureMut {
            data: vec![1, 2, 3],
            f: |v| {
                v.push(4);
                v.len()
            },
        };
        let cloned = closure.clone();
        let mut c1 = closure;
        let mut c2 = cloned;

        assert_eq!(c1.call_mut(), 4);
        assert_eq!(c1.call_mut(), 5); // Continues to grow
        assert_eq!(c2.call_mut(), 4); // Independent copy
    }

    #[test]
    fn test_closure_mut_thread_aware() {
        let affinities = pinned_affinities(&[2, 3]);

        // Test with i32 - mutating state across relocations
        let mut closure = closure_mut(0_i32, |x| {
            *x += 1;
            *x
        });
        closure.relocate(Some(affinities[0]), affinities[2]);
        assert_eq!(closure.call_mut(), 1);
        assert_eq!(closure.call_mut(), 2);

        // Test with String - mutating string state
        let mut closure = closure_mut(String::new(), |s| {
            s.push('x');
            s.len()
        });

        closure.relocate(Some(affinities[0]), affinities[2]);
        assert_eq!(closure.call_mut(), 1);
        assert_eq!(closure.call_mut(), 2);
        assert_eq!(closure.call_mut(), 3);
    }

    #[test]
    fn test_closure_mut_relocate_fn_mut() {
        let mut closure = closure_mut(vec![1, 2], |v| {
            v.push(v.len() + 1);
            v.len()
        });

        assert_eq!(closure.call_mut(), 3);
        assert_eq!(closure.call_mut(), 4);
        assert_eq!(closure.call_mut(), 5);
    }

    #[test]
    fn test_closure_mut_relocate_fn_mut_independent_after_clone() {
        let closure = closure_mut(0_i32, |x| {
            *x += 10;
            *x
        });

        let mut c1 = closure;
        let mut c2 = closure;

        assert_eq!(c1.call_mut(), 10);
        assert_eq!(c1.call_mut(), 20);
        assert_eq!(c2.call_mut(), 10);
        assert_eq!(c2.call_mut(), 20);
    }

    #[test]
    fn test_closure_mut_relocate_fn_once() {
        let closure = closure_mut(String::from("test"), |s| {
            s.push('!');
            s.clone()
        });

        let result = closure.call_once();
        assert_eq!(result, "test!");
    }

    // Integration tests combining traits

    #[test]
    fn test_closure_all_traits_together() {
        let affinities = pinned_affinities(&[2]);
        let c = closure(vec![1, 2, 3], std::vec::Vec::len);

        // Test Clone
        let mut cloned = c;

        // Test ThreadAware
        cloned.relocate(Some(affinities[0]), affinities[1]);

        // Test ThreadAwareFnMut
        assert_eq!(cloned.call_mut(), 3);
    }

    #[test]
    fn test_closure_mut_all_traits_together() {
        let affinities = pinned_affinities(&[2, 2]);
        let closure = closure_mut(100_i32, |x| {
            *x += 1;
            *x
        });

        // Test Clone
        let mut cloned = closure;

        // Test ThreadAware across NUMA nodes
        cloned.relocate(Some(affinities[0]), affinities[3]);

        // Test ThreadAwareFnMut
        assert_eq!(cloned.call_mut(), 101);
        assert_eq!(cloned.call_mut(), 102);
    }

    #[test]
    fn test_closure_once_with_thread_aware_and_clone() {
        let affinities = pinned_affinities(&[2]);
        let closure = closure_once((1, 2, 3), |(a, b, c)| a + b + c);

        // Test Clone
        let mut cloned = closure;

        // Test ThreadAware
        cloned.relocate(Some(affinities[0]), affinities[1]);

        // Call once
        assert_eq!(cloned.call_once(), 6);

        // Original can still be used
        assert_eq!(closure.call_once(), 6);
    }

    // ---- Mutation-detecting tests ----
    // These use a Tracker type that visibly changes on relocate, ensuring
    // mutation testing catches no-op replacements of relocate bodies.

    #[derive(Clone, Debug, PartialEq)]
    struct Tracker(bool);

    impl ThreadAware for Tracker {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.0 = true;
        }
    }

    fn affinities() -> (Option<Affinity>, Affinity) {
        let a = pinned_affinities(&[2]);
        (Some(a[0]), a[1])
    }

    #[test]
    fn closure_relocate_forwards_to_data() {
        let (src, dst) = affinities();
        let mut c = closure(Tracker(false), |t| t.0);
        c.relocate(src, dst);
        assert!(c.call(), "Closure must forward relocate to captured data");
    }

    #[test]
    fn closure_once_relocate_forwards_to_data() {
        let (src, dst) = affinities();
        let mut c = closure_once(Tracker(false), |t| t.0);
        c.relocate(src, dst);
        assert!(c.call_once(), "ClosureOnce must forward relocate to captured data");
    }

    #[test]
    fn closure_mut_relocate_forwards_to_data() {
        let (src, dst) = affinities();
        let mut c = closure_mut(Tracker(false), |t| t.0);
        c.relocate(src, dst);
        assert!(c.call_mut(), "ClosureMut must forward relocate to captured data");
    }

    #[test]
    fn async_closure_relocate_forwards_to_data() {
        let (src, dst) = affinities();
        let mut c = async_closure(Tracker(false), |t| Box::pin(async move { t.0 }));
        c.relocate(src, dst);
        let result = futures::executor::block_on(c.call());
        assert!(result, "AsyncClosure must forward relocate to captured data");
    }

    #[test]
    fn async_closure_once_relocate_forwards_to_data() {
        let (src, dst) = affinities();
        let mut c = async_closure_once(Tracker(false), |t| Box::pin(async move { t.0 }));
        c.relocate(src, dst);
        let result = futures::executor::block_on(c.call_once());
        assert!(result, "AsyncClosureOnce must forward relocate to captured data");
    }

    #[test]
    fn async_closure_mut_relocate_forwards_to_data() {
        let (src, dst) = affinities();
        let mut c = async_closure_mut(Tracker(false), |t| {
            let val = t.0;
            Box::pin(async move { val })
        });
        c.relocate(src, dst);
        let result = futures::executor::block_on(c.call_mut());
        assert!(result, "AsyncClosureMut must forward relocate to captured data");
    }

    // ---- Coverage for Debug, Clone, call_once, trait impls on async closure types ----

    #[test]
    fn closure_call_once_trait() {
        // Exercises ThreadAwareFnOnce::call_once for Closure (line 109-111)
        let c = closure(42_i32, |x| *x + 1);
        assert_eq!(ThreadAwareFnOnce::call_once(c), 43);
    }

    #[test]
    fn async_closure_debug_and_clone() {
        let c = async_closure(String::from("hello"), |x| Box::pin(async move { x.len() }));
        let dbg = format!("{c:?}");
        assert!(dbg.contains("AsyncClosure"), "{dbg}");
        let c2 = c.clone();
        let r = futures::executor::block_on(c2.call());
        assert_eq!(r, 5);
        // Use original after clone to prove clone is not redundant
        let r2 = futures::executor::block_on(c.call());
        assert_eq!(r2, 5);
    }

    #[test]
    fn async_closure_trait_impls() {
        // ThreadAwareAsyncFn::call
        let c = async_closure(10_i32, |x| Box::pin(async move { *x }));
        let c: Box<dyn ThreadAwareAsyncFn<i32>> = Box::new(c);
        let r = futures::executor::block_on(c.call());
        assert_eq!(r, 10);

        // ThreadAwareAsyncFnMut::call_mut
        let mut c = async_closure(10_i32, |x| Box::pin(async move { *x }));
        let r = futures::executor::block_on(ThreadAwareAsyncFnMut::call_mut(&mut c));
        assert_eq!(r, 10);

        // ThreadAwareAsyncFnOnce::call_once (Box<Self>)
        let c = async_closure(10_i32, |x| Box::pin(async move { *x }));
        let boxed: Box<dyn ThreadAwareAsyncFnOnce<i32>> = Box::new(c);
        let r = futures::executor::block_on(boxed.call_once());
        assert_eq!(r, 10);
    }

    #[test]
    fn async_closure_once_debug_and_clone() {
        let c = async_closure_once(String::from("hello"), |x| Box::pin(async move { x }));
        let dbg = format!("{c:?}");
        assert!(dbg.contains("AsyncClosureOnce"), "{dbg}");
        let c2 = c.clone();
        let r = futures::executor::block_on(c2.call_once());
        assert_eq!(r, "hello");
        // Use original after clone to prove clone is not redundant
        let r2 = futures::executor::block_on(c.call_once());
        assert_eq!(r2, "hello");
    }

    #[test]
    fn async_closure_once_trait_call_once() {
        // ThreadAwareAsyncFnOnce::call_once (Box<Self>)
        let c = async_closure_once(99_i32, |x| Box::pin(async move { x }));
        let boxed: Box<dyn ThreadAwareAsyncFnOnce<i32>> = Box::new(c);
        let r = futures::executor::block_on(boxed.call_once());
        assert_eq!(r, 99);
    }

    #[test]
    fn async_closure_mut_debug_and_clone() {
        let c = async_closure_mut(String::from("hello"), |x| {
            let v = x.clone();
            Box::pin(async move { v })
        });
        let dbg = format!("{c:?}");
        assert!(dbg.contains("AsyncClosureMut"), "{dbg}");
        let mut c2 = c.clone();
        let r = futures::executor::block_on(c2.call_mut());
        assert_eq!(r, "hello");
        // Use original after clone to prove clone is not redundant
        drop(c);
    }

    #[test]
    fn async_closure_mut_trait_impls() {
        // ThreadAwareAsyncFnMut::call_mut
        let mut c = async_closure_mut(10_i32, |x| {
            let v = *x;
            Box::pin(async move { v })
        });
        let r = futures::executor::block_on(ThreadAwareAsyncFnMut::call_mut(&mut c));
        assert_eq!(r, 10);

        // ThreadAwareAsyncFnOnce::call_once (Box<Self>)
        let c = async_closure_mut(10_i32, |x| {
            let v = *x;
            Box::pin(async move { v })
        });
        let boxed: Box<dyn ThreadAwareAsyncFnOnce<i32>> = Box::new(c);
        let r = futures::executor::block_on(boxed.call_once());
        assert_eq!(r, 10);
    }
}

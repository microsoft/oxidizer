// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub mod erased;

use crate::Transfer;

pub use erased::ErasedClosureOnce;

/// A trait for callable types that can be called once.
///
/// This trait is used to define closures that can be called once, consuming the closure itself.
/// It is similar to the `FnOnce` trait in Rust, but it is designed to work with the `Transfer` trait.
pub trait TransferFnOnce<T: ?Sized>: Transfer {
    fn call_once(self) -> T;
}

/// A trait for callable types that can be called multiple times.
/// This trait is used to define closures that can be called multiple times, without consuming the closure.
pub trait TransferFn<T>: Transfer {
    fn call(&self) -> T;
}

/// A trait for callable types that can be called mutably.
/// This trait is used to define closures that can be called mutably, allowing the closure to modify its internal state.
pub trait TransferFnMut<T>: Transfer {
    fn call_mut(&mut self) -> T;
}

#[derive(Debug, Copy, PartialEq, Eq, Hash)]
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

impl<T, D> TransferFn<T> for Closure<T, D>
where
    D: Transfer,
{
    fn call(&self) -> T {
        (self.f)(&self.data)
    }
}

impl<T, D> TransferFnMut<T> for Closure<T, D>
where
    D: Transfer,
{
    fn call_mut(&mut self) -> T {
        self.call()
    }
}

impl<T, D> TransferFnOnce<T> for Closure<T, D>
where
    D: Transfer,
{
    fn call_once(self) -> T {
        self.call()
    }
}

impl<T, D> Transfer for Closure<T, D>
where
    D: Transfer,
{
    async fn transfer(self, source: crate::Domain, destination: crate::Domain) -> Self {
        let data = self.data.transfer(source, destination).await;
        Self { data, f: self.f }
    }
}

#[derive(Debug, Copy, PartialEq, Eq, Hash)]
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

impl<T, D> TransferFnOnce<T> for ClosureOnce<T, D>
where
    D: Transfer,
{
    fn call_once(self) -> T {
        (self.f)(self.data)
    }
}

impl<T, D> Transfer for ClosureOnce<T, D>
where
    D: Transfer,
{
    async fn transfer(self, source: crate::Domain, destination: crate::Domain) -> Self {
        let data = self.data.transfer(source, destination).await;
        Self { data, f: self.f }
    }
}

#[derive(Debug, Copy, PartialEq, Eq, Hash)]
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

impl<T, D> TransferFnMut<T> for ClosureMut<T, D>
where
    D: Transfer,
{
    fn call_mut(&mut self) -> T {
        (self.f)(&mut self.data)
    }
}

impl<T, D> TransferFnOnce<T> for ClosureMut<T, D>
where
    D: Transfer,
{
    fn call_once(mut self) -> T {
        self.call_mut()
    }
}

impl<T, D> Transfer for ClosureMut<T, D>
where
    D: Transfer,
{
    async fn transfer(self, source: crate::Domain, destination: crate::Domain) -> Self {
        let data = self.data.transfer(source, destination).await;
        Self { data, f: self.f }
    }
}

pub fn closure<T, D>(data: D, f: fn(&D) -> T) -> Closure<T, D>
where
    D: Transfer,
{
    Closure { data, f }
}

pub fn closure_mut<T, D>(data: D, f: fn(&mut D) -> T) -> ClosureMut<T, D>
where
    D: Transfer,
{
    ClosureMut { data, f }
}

pub fn closure_once<T, D>(data: D, f: fn(D) -> T) -> ClosureOnce<T, D>
where
    D: Transfer,
{
    ClosureOnce { data, f }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxed_once() {
        let x = closure_once(42, |x| x + 1);
        let y = Box::new(x);
        let _z = y.call_once();
    }

    #[test]
    fn more_stuff() {
        let x = Closure {
            data: 42,
            f: |x| x + 1,
        };

        let _y = x.call();
    }

    #[test]
    fn something() {
        fn takes_mut(mut x: impl TransferFnMut<i32>) {
            let _y = x.call_mut();
        }

        let x = closure(42, |x| x + 1);
        takes_mut(x);
    }

    #[allow(
        clippy::empty_structs_with_brackets,
        reason = " Testing non-clone behavior"
    )]
    #[test]
    fn non_clone() {
        struct MyStruct {}

        let y = closure((), |()| MyStruct {});
        let _z = y.call();
    }

    #[allow(clippy::redundant_clone, reason = "Testing clone behavior")]
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
}
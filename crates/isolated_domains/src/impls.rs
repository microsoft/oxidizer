// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{path::PathBuf, time::Duration};

use crate::transfer::{Domain, Transfer};

// To make impl_transfer(...) work
macro_rules! impl_transfer {
    ($t:ty) => {
        impl Transfer for $t {
            async fn transfer(self, _source: Domain, _destination: Domain) -> Self {
                self
            }
        }
    };
}

impl_transfer!(bool);
impl_transfer!(u8);
impl_transfer!(u16);
impl_transfer!(u32);
impl_transfer!(u64);
impl_transfer!(i8);
impl_transfer!(i16);
impl_transfer!(i32);
impl_transfer!(i64);
impl_transfer!(usize);
impl_transfer!(isize);
impl_transfer!(f32);
impl_transfer!(f64);
impl_transfer!(char);

impl_transfer!(String);
impl_transfer!(PathBuf);
impl_transfer!(Duration);

impl_transfer!(&'static str);

// We need to implement `Transfer` for tuples ranging from 0 to 12 elements
macro_rules! impl_transfer_tuple {
    ($head:ident, $($tail:ident,)*) => {
        impl<$head, $($tail),*> Transfer for ($head, $($tail),*)
            where
                $head: Transfer,
                $($tail: Transfer),*
                {
                    async fn transfer(self, source: Domain, destination: Domain) -> Self {
                        #[allow(non_snake_case, reason = "Macro-generated code uses uppercase identifiers for tuple elements")]
                        let ($head, $($tail),*) = self;
                        (
                            $head.transfer(source, destination).await,
                            $( $tail.transfer(source, destination).await, )*
                        )
                    }
                }

                // Recursively call the macro for the rest of the tuple
                impl_transfer_tuple!($($tail,)*);
    };

    () => {
        impl Transfer for () {
            async fn transfer(self, _source: Domain, _destination: Domain) -> Self {
                self
            }
        }
    };
}

impl_transfer_tuple!(A, B, C, D, E, F, G, H, I, J, K, L,);

//TODO impl_transfer_array! macro to implement Transfer for arrays

impl<T> Transfer for Option<T>
where
    T: Transfer,
{
    async fn transfer(self, source: Domain, destination: Domain) -> Self {
        match self {
            Some(value) => Some(value.transfer(source, destination).await),
            None => None,
        }
    }
}

impl<T, E> Transfer for Result<T, E>
where
    T: Transfer,
    E: Transfer,
{
    async fn transfer(self, source: Domain, destination: Domain) -> Self {
        match self {
            Ok(value) => Ok(value.transfer(source, destination).await),
            Err(err) => Err(err.transfer(source, destination).await),
        }
    }
}

impl<T> Transfer for Vec<T>
where
    T: Transfer,
{
    async fn transfer(self, source: Domain, destination: Domain) -> Self {
        let mut result = Self::with_capacity(self.len());
        for value in self {
            result.push(value.transfer(source, destination).await);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::Transfer;
    use oxidizer_rt::{BasicThreadState, Runtime};
    use oxidizer_testing::execute_or_abandon;

    #[cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
    #[test]
    fn test_option() {
        let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

        let mut handle = runtime.spawn(async move |_cx| {
            let domains = crate::create_domains(2);

            let value: Option<i32> = Some(42);
            let transferred = value.transfer(domains[0], domains[1]).await;
            assert_eq!(transferred, Some(42));

            let none_value: Option<i32> = None;
            let transferred_none = none_value.transfer(domains[0], domains[1]).await;
            assert_eq!(transferred_none, None);
        });

        execute_or_abandon(move || handle.wait()).unwrap();
    }

    #[cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
    #[test]
    fn test_vec() {
        let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

        let mut handle = runtime.spawn(async move |_cx| {
            let domains = crate::create_domains(2);

            let value: Vec<i32> = vec![1, 2, 3];
            let transferred = value.transfer(domains[0], domains[1]).await;
            assert_eq!(transferred, vec![1, 2, 3]);

            let empty_value: Vec<i32> = Vec::<i32>::new();
            let transferred_empty = empty_value.transfer(domains[0], domains[1]).await;
            assert_eq!(transferred_empty, Vec::<i32>::new());
        });

        execute_or_abandon(move || handle.wait()).unwrap();
    }
}
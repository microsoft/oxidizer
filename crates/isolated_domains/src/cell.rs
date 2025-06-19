// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod storage;

use std::{
    ops::Deref,
    pin::Pin,
    sync::{Arc, RwLock},
};

use crate::{Domain, Transfer, TransferFnOnce, closure::ErasedClosureOnce, closure_once};
use storage::Storage;

/// Transferable reference counted type.
///
/// This type works like `Arc`, but it allows transferring the data between different domains.
/// Each domain gets its own copy of the data, and the data is not shared between domains.
///
/// # Clone behavior
/// Cloning a `Trc<T>` will behave like `Arc<T>`, meaning it will create a new reference to the same data.
///
/// # Transfer behavior
/// When transferring a `Trc<T>` from one domain to another, it will check if the data already exists in the destination domain.
/// If it does, it will use the existing data otherwise it will recreate the data using the stored factory closure or clone of the data.
#[derive(Debug, Clone)]
pub struct Trc<T> {
    storage: Arc<RwLock<Storage<Arc<T>>>>,
    data: Arc<T>,
    factory: Factory<T>,
}

type DataFn<T> = fn(&T, Domain, Domain) -> Pin<Box<dyn Future<Output = T>>>;

#[derive(Debug, Clone)]
enum Factory<T> {
    /// An external closure was provided to create the data.
    Closure(Arc<ErasedClosureOnce<T>>),

    /// An external closure was provided to create the data.
    AsyncClosure(Arc<ErasedClosureOnce<Pin<Box<dyn Future<Output = T>>>>>),

    /// The data is Transfer + Clone and will be cloned and transferred.
    Data(DataFn<T>),
}

impl<T> Deref for Trc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> Trc<T>
where
    T: Transfer + Clone + 'static,
{
    /// Creates a new `Trc` with the given value.
    /// The value must implement `Transfer` and `Clone`.
    pub fn new(value: T) -> Self {
        let data = Arc::new(value);

        Self {
            storage: Arc::new(RwLock::new(storage::Storage::new())),
            data,
            factory: Factory::Data(|data: &T, source, destination| {
                let data = data.clone();
                Box::pin(async move { data.transfer(source, destination).await })
            }),
        }
    }
}

impl<T> Trc<T>
where
    T: 'static,
{
    /// Creates a new `Trc` with a closure that will be called once to create the data.
    ///
    /// # Notes on transferring
    /// Clones of this closure will be transferred between domains and might be called with different source domains,
    /// the original will never be transferred.
    pub fn with_closure<F>(closure: F) -> Self
    where
        F: TransferFnOnce<T> + Clone + Transfer + 'static,
    {
        let data = Arc::new(closure.clone().call_once());

        Self {
            storage: Arc::new(RwLock::new(storage::Storage::new())),
            data,
            factory: Factory::Closure(Arc::new(ErasedClosureOnce::new(closure))),
        }
    }

    /// Creates a new `Trc` with a closure that will be called once to create the data.
    ///
    /// # Notes on transferring
    /// Clones of this closure will be transferred between domains and might be called with different source domains,
    /// the original will never be transferred.
    pub async fn with_async_closure<F, X>(closure: F) -> Self
    where
        X: Future<Output = T> + 'static,
        F: TransferFnOnce<X> + Clone + Transfer + 'static,
    {
        let data = Arc::new(closure.clone().call_once().await);

        let closure = closure_once(closure, |closure| -> Pin<Box<dyn Future<Output = T>>> {
            Box::pin(async move { closure.call_once().await })
        });

        Self {
            storage: Arc::new(RwLock::new(storage::Storage::new())),
            data,
            factory: Factory::AsyncClosure(Arc::new(ErasedClosureOnce::new(closure))),
        }
    }
}

impl<T> Trc<T> {
    /// Converts the `Trc<T>` into an `Arc<T>`.
    #[must_use]
    pub fn into_arc(self) -> Arc<T> {
        self.data
    }
}

impl<T> Transfer for Trc<T> {
    async fn transfer(self, source: Domain, destination: Domain) -> Self {
        // let mut write = self.storage.write().expect("Failed to acquire write lock");

        let data = self
            .storage
            .read()
            .expect("Failed to acquire read lock")
            .get_clone(destination);

        let data = if let Some(data) = data {
            data
        } else {
            // We need to transfer or recreate the data
            let data = match &self.factory {
                // We can use the closure to create new data
                Factory::Closure(factory) => {
                    let factory = (**factory).clone();
                    factory.transfer(source, destination).await.call_once()
                }

                Factory::AsyncClosure(factory) => {
                    let factory = (**factory).clone();
                    factory
                        .transfer(source, destination)
                        .await
                        .call_once()
                        .await
                }

                // We can clone and transfer the data
                Factory::Data(factory) => factory(&self.data, source, destination).await,
            };

            let data = Arc::new(data);

            let old_data = self
                .storage
                .write()
                .expect("Failed to acquire write lock")
                .replace(destination, Arc::<T>::clone(&data));

            assert!(
                old_data.is_none(),
                "Data already exists for the destination domain"
            );

            data
        };

        self.storage
            .write()
            .expect("Failed to acquire write lock")
            .replace(source, self.data);

        Self {
            storage: self.storage,
            data,
            factory: self.factory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Trc;
    use crate::Inert;
    use crate::Transfer;
    use crate::closure;
    use crate::closure_once;
    use crate::create_domains;

    #[cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
    #[allow(clippy::redundant_clone, reason = "Testing clone behavior")]
    #[oxidizer_rt::test]
    async fn test_nested_trc(_context: oxidizer_rt::BasicThreadState) {
        let domains = create_domains(3);
        let trc = Trc::new(42);
        let nested_trc = Trc::with_closure(closure_once(trc, |_trc| 123));

        let nested_trc = nested_trc.transfer(domains[0], domains[1]).await;

        let cloned_trc = nested_trc.clone();
        let cloned_trc = cloned_trc.transfer(domains[1], domains[2]).await;

        let _ = cloned_trc;
    }

    #[allow(clippy::redundant_clone, reason = "Testing clone behavior")]
    #[test]
    fn test_trc_clone() {
        let value = Trc::new(42);
        let cloned_value = value.clone();
        assert_eq!(*value, *cloned_value);
    }

    #[cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
    #[oxidizer_rt::test]
    async fn test_trc_transfer(_context: oxidizer_rt::BasicThreadState) {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNT: AtomicU32 = AtomicU32::new(0);

        let domains = create_domains(2);

        let closure = closure((), |()| COUNT.fetch_add(1, Ordering::SeqCst));
        let trc = Trc::with_closure(closure);

        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
        assert_eq!(*trc, 0);

        let transferred = trc.transfer(domains[0], domains[1]).await;

        assert_eq!(COUNT.load(Ordering::SeqCst), 2);
        assert_eq!(*transferred, 1);
    }

    #[test]
    fn test_into_arc() {
        let trc = Trc::with_closure(closure((), |()| 42));
        let _arc = trc.into_arc();

        let trc = Trc::new(42);
        let _arc = trc.into_arc();

        let trc = Trc::new(Inert(42));
        let _arc = trc.into_arc();
    }

    #[test]
    fn test_from() {
        let trc = Trc::with_closure(closure((), |()| 42));
        let _arc = trc.into_arc();

        let trc = Trc::new(42);
        let _arc = trc.into_arc();

        let trc = Trc::new(Inert(42));
        let _arc = trc.into_arc().into_inner();
    }
}
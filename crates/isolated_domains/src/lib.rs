// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A library for creating isolated domains in Rust, allowing for safe and efficient data transfer between them.
//!
//! This is useful in scenarios where you want to isolate data to different domains, such as NUMA nodes, threads, or specific CPU cores.
//! It can serve as a foundation for building a async runtime or a task scheduler that operates across multiple domains.
//! For example it can be used to implement a NUMA-aware task scheduler that transfers data between different NUMA nodes
//! or a thread-per-core scheduler that transfers data between different CPU cores.
//!
//! The way this would be used is by restricting how work can be scheduled on other domains (threads,
//! NUMA nodes). If the runtime only allows work scheduling in a way that accepts work that can be
//! transferred (e.g. by using the [`TransferFnOnce`] trait) and makes sure that transfer is called, it can
//! effectively isolate the domains as the [`Transfer`] trait ensures the right level of separation if
//! implemented correctly.
//!
//! Transfer is an 'infectious' trait, meaning that when you implement it for a type,
//! all of its fields must also implement [`Transfer`] and you must call their `transfer` methods.
//! However [`Transfer`] is provided for many common types, so you can use it out of the box for most cases.
//!
//! # Example
//!
//! ```rust
//! use isolated_domains::{create_domains, Domain, Transfer, Inert};
//!
//!
//! // Define a type that implements Transfer
//! #[derive(Debug, Clone)]
//! struct MyData {
//!    value: i32,
//! }
//!
//! impl Transfer for MyData {
//!    async fn transfer(mut self, source: Domain, destination: Domain) -> Self {
//!       self.value = self.value.transfer(source, destination).await;
//!       self
//!    }
//! }
//!
//! async fn do_transfer() {
//!     //! // Create two domains
//!     let domains = create_domains(2);
//!
//!     // Create an instance of MyData
//!     let data = MyData { value: 42 };
//!
//!     // Transfer data from one domain to another
//!     let transferred_data = data.transfer(domains[0], domains[1]).await;
//!
//!     // Use Inert to create a type that does not transfer data
//!     struct MyInertData(i32);
//!
//!     let inert_data = Inert(MyInertData(100));
//!     let transferred_inert_data = inert_data.transfer(domains[0], domains[1]).await;
//! }
//! ```

mod cell;
mod closure;
mod impls;
mod transfer;
mod wrappers;

pub use cell::Trc;
pub use closure::{
    Closure, ClosureMut, ClosureOnce, TransferFn, TransferFnMut, TransferFnOnce, closure,
    closure_mut, closure_once,
};
pub use transfer::{Domain, Transfer, create_domains};
pub use wrappers::Inert;
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/cancelable/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/cancelable/favicon.ico")]

//! Cooperative cancellation via tokens.
//!
//! This module provides [`CancellationTokenSource`] and [`CancellationToken`],
//! modeled after the equivalent C# types. A source controls cancellation and
//! hands out lightweight, cloneable tokens for observers to check.
//!
//! # Linked Sources
//!
//! A linked source cancels when *any* of its parent tokens are canceled,
//! enabling composition of multiple cancellation signals:
//!
//! ```
//! # fn example() {
//! use cancelable::CancellationTokenSource;
//!
//! let first = CancellationTokenSource::new();
//! let second = CancellationTokenSource::new();
//!
//! let linked = CancellationTokenSource::linked(&[first.token(), second.token()]);
//! let token = linked.token();
//!
//! assert!(!token.is_cancelled());
//! second.cancel();
//! assert!(token.is_cancelled());
//! # }
//! ```
//!
//! # Subscribers
//!
//! Register callbacks that fire exactly once when cancellation occurs:
//!
//! ```
//! # fn example() {
//! use cancelable::CancellationTokenSource;
//!
//! let source = CancellationTokenSource::new();
//! source.subscribe(|| println!("Operation canceled"));
//! source.cancel();
//! # }
//! ```
//!
//! # Futures
//!
//! The [`CancelableExt`] trait adds a [`cancelable`](CancelableExt::cancelable) method
//! to any [`Future`], pairing it with a [`CancellationToken`] so that each
//! poll checks for cancellation before and after driving the inner future.
//!
//! ```
//! # async fn example() -> Result<(), ohno::AppError> {
//! use cancelable::{CancelableExt, CancellationTokenSource};
//!
//! let source = CancellationTokenSource::new();
//! let token = source.token();
//!
//! let result = async { 42 }.cancelable(token).await?;
//! assert_eq!(result, 42);
//! # Ok(())
//! # }
//! ```

pub mod future;
pub use future::CancelableExt;

mod token;
pub use token::{CancellationToken, CancellationTokenSource};

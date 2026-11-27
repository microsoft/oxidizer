// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Actor message marker trait.
//!
//! `Message` marks values that can be delivered to an actor inbox. Most actors
//! use one enum to describe the messages they accept.

/// Marker trait for values that can be delivered to an actor inbox.
///
/// Enactor implements `Message` for every `Send + 'static` type, though most
/// actors use one enum to describe the messages they accept.
pub trait Message: Send + 'static {}

impl<T: Send + 'static> Message for T {}

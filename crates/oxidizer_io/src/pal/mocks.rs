// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;
use std::num::NonZeroUsize;
use std::pin::Pin;

use mockall::mock;

use crate::pal::{
    CompletionNotification, CompletionQueueFacade, ElementaryOperation, ElementaryOperationFacade,
    ElementaryOperationKey, MemoryPool, MemoryPoolFacade, Platform, Primitive,
};

mock! {
    #[derive(Debug)]
    pub ElementaryOperation {
        /// Used in tests to inspect what offset the elementary operation was created for.
        pub fn offset(&self) -> u64;
     }

    impl ElementaryOperation for ElementaryOperation {
        fn key(self: Pin<&Self>) -> ElementaryOperationKey;
    }
}

mock! {
    #[derive(Debug)]
    pub CompletionNotification { }

    impl CompletionNotification for CompletionNotification {
        fn is_wake_up_signal(&self) -> bool;

        fn elementary_operation_key(&self) -> ElementaryOperationKey;

        fn result(&self) -> crate::Result<u32>;
    }
}

mock! {
    #[derive(Debug)]
    pub Primitive {
        /// Can be used in tests to verify which MockPrimitive was returned to test code.
        /// Equivalent to .as_handle() etc on primitives specialized for other platforms.
        pub fn as_raw(&self) -> u64;
    }

    impl Primitive for Primitive {
        fn close(&self);
    }

    impl Clone for Primitive {
        fn clone(&self) -> Self;
    }

    impl Display for Primitive {
        fn fmt<'a>(&self, f: &mut std::fmt::Formatter<'a>) -> std::fmt::Result;
    }
}

mock! {
    #[derive(Debug)]
    pub MemoryPool {}

    impl MemoryPool for MemoryPool {
        fn rent(
            &self,
            count_bytes: usize,
            preferred_block_size: NonZeroUsize,
        ) -> crate::mem::SequenceBuilder;
    }
}

mock! {
    #[derive(Debug)]
    pub Platform { }

    impl Platform for Platform {
        fn new_completion_queue(&self) -> CompletionQueueFacade;
        fn new_elementary_operation(&self, offset: u64) -> ElementaryOperationFacade;
        fn new_memory_pool(&self) -> MemoryPoolFacade;
    }
}
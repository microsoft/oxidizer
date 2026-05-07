// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "This is a test module")]

use thread_aware::{Arc, PerCore, ThreadAware};

#[test]
fn supports_dyn_trait() {
    _ = Arc::<dyn ThreadAware, PerCore>::new_boxed(|| Box::new(String::new()));
    _ = Arc::<dyn ThreadAware, PerCore>::with_clone_fn(String::new(), |x: &String| Box::new(x.clone()));
}

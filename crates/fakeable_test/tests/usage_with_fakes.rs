// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use fakeable_test::my_service::MyService;
use fakeable_test::my_service::fakes::FakeMyService;
use static_assertions::assert_impl_all;
use thread_aware::ThreadAware;

#[tokio::test]
async fn test_real_implementation() {
    let mut service = MyService::new("real".to_string(), 10);
    assert_eq!(service.get_value(), "real");
    assert_eq!(service.get_other_value(), 10);
    assert_eq!(service.process("Prefix"), "Prefix Value: real, Other: 10");
    assert_eq!(service.async_function(5).await.unwrap(), 5);
    service.mutable(5);
    assert_eq!(service.get_other_value(), 15);
    assert_eq!(service.get_value_with_ref_arg("pre-", Some("post")), "pre-realpost");
}

#[tokio::test]
async fn test_fake_implementation() {
    let mut fake_service = MyService::fake(FakeMyService);
    assert_eq!(fake_service.get_value(), "fake");
    assert_eq!(fake_service.get_other_value(), 42);
    assert_eq!(fake_service.process("Prefix"), "processed Prefix");
    assert_eq!(fake_service.async_function(5).await.unwrap(), 47);
    fake_service.mutable(5);
    assert_eq!(fake_service.get_other_value(), 42); // Fake does not change state
    assert_eq!(fake_service.get_value_with_ref_arg("pre-", None), "pre-fake");
}

assert_impl_all!(MyService: ThreadAware, Clone);

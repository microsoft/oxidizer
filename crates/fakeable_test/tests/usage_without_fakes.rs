// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use fakeable_test::my_service_unused_fake::MyService;
use fakeable_test::test_only_service::Service;
use futures::executor::block_on;
use static_assertions::assert_impl_all;

assert_impl_all!(Service: Send, Sync, Clone);

#[test]
fn test_real_implementation() {
    let service = MyService::new("real".to_string(), 10);
    assert_eq!(service.get_value(), "real");
    assert_eq!(service.get_other_value(), 10);
    assert_eq!(service.process("Prefix"), "Prefix Value: real, Other: 10");
    assert_eq!(block_on(service.async_function(5)).unwrap(), 5);
}

#[test]
fn test_only_service_stays_real_when_used_as_a_dependency() {
    let service = Service::new(3);
    let changed = service.with_value(5);

    assert_eq!(service.value(), 3);
    assert_eq!(changed.value(), 5);
}

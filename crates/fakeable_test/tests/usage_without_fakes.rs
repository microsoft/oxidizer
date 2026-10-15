// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use fakeable_test::my_service_unused_fake::MyService;
use futures::executor::block_on;

#[test]
fn test_real_implementation() {
    let service = MyService::new("real".to_string(), 10);
    assert_eq!(service.get_value(), "real");
    assert_eq!(service.get_other_value(), 10);
    assert_eq!(service.process("Prefix"), "Prefix Value: real, Other: 10");
    assert_eq!(block_on(service.async_function(5)).unwrap(), 5);
}

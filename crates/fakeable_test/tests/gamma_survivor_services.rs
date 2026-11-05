// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Observable real and fake service forwarding contracts.

use fakeable_test::my_service::MyService;
use fakeable_test::my_service::fakes::FakeMyService;
use fakeable_test::my_service_mockall::MyService as MockableService;
use fakeable_test::my_service_unused_fake::fakes::FakeMyService as UnusedFake;
use futures::executor::block_on;

#[test]
fn gamma_survivor_clone_transform_consume_and_optional_suffix() {
    let original = MyService::new("payload".into(), 17);
    let cloned = original.special_clone(6);
    assert_eq!(original.get_other_value(), 17);
    assert_eq!(cloned.get_other_value(), 23);
    assert_eq!(cloned.get_value(), "payload");
    let transformed = cloned.transform(-4);
    assert_eq!(transformed.consume(), "Value: payload, Other: 19");
    assert_eq!(original.consume(), "Value: payload, Other: 17");

    let fake = MyService::fake(FakeMyService);
    assert_eq!(fake.get_value_with_ref_arg("left/", Some("/right")), "left/fake/right");
    assert_eq!(fake.consume(), "consumed");

    let real = MockableService::new("center".into(), 13);
    assert_eq!(real.get_value_with_ref_arg("left/", Some("/right")), "left/center/right");
    assert_eq!(real.get_value_with_ref_arg("left/", None), "left/center");
}

#[test]
fn gamma_survivor_unused_fake_is_directly_callable_and_preserves_arguments() {
    let fake = UnusedFake;
    assert_eq!(fake.process("prefix"), "processed prefix");
    for input in [-9, 0, 7] {
        assert_eq!(block_on(fake.async_function(input)).unwrap(), 42 + input);
    }
}

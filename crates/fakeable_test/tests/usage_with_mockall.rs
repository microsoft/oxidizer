// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use fakeable_test::my_service_mockall::MyService;
use fakeable_test::my_service_mockall::fakes::MockMyService;
use static_assertions::assert_impl_all;
use thread_aware::{ThreadAware, Unaware};

#[tokio::test]
async fn test_mock_implementation() {
    let mut mock = MockMyService::new();
    mock.expect_get_value().return_const("Test".into());
    mock.expect_get_value_with_ref_arg()
        .returning(|prefix, suffix| format!("{}mocked{}", prefix, suffix.unwrap_or("")));
    mock.expect_get_other_value().return_const(43);
    mock.expect_async_function().returning(|_x| Box::pin(async { Ok(44i32) }));

    let service = MyService::fake(Unaware(mock.into()));
    assert_eq!(service.get_value(), "Test");
    assert_eq!(service.get_value_with_ref_arg("pre-", Some("post")), "pre-mockedpost");
    assert_eq!(service.get_value_with_ref_arg("pre-", None), "pre-mocked");
    assert_eq!(service.get_other_value(), 43);
    assert_eq!(service.async_function(5).await.unwrap(), 44);
}

#[tokio::test]
async fn test_real_implementation() {
    let service = MyService::new("real".to_string(), 10);
    assert_eq!(service.get_value(), "real");
    assert_eq!(service.get_other_value(), 10);
    assert_eq!(service.async_function(5).await.unwrap(), 5);
}

assert_impl_all!(MyService: ThreadAware, Clone);

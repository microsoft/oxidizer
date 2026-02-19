// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for [`Merger::execute()`].

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{AcqRel, Acquire};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use uniflight::Merger;

fn unreachable_future() -> std::future::Pending<String> {
    std::future::pending()
}

#[tokio::test]
async fn direct_call() {
    let group = Merger::<String, String, _>::new_per_process();
    let result = group
        .execute("key", || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            "Result".to_string()
        })
        .await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn parallel_call() {
    let call_counter = AtomicUsize::default();

    let group = Merger::<String, String, _>::new_per_process();
    let futures = FuturesUnordered::new();
    for _ in 0..10 {
        futures.push(group.execute("key", || async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            call_counter.fetch_add(1, AcqRel);
            "Result".to_string()
        }));
    }

    assert!(futures.all(|out| async move { out == Ok("Result".to_string()) }).await);
    assert_eq!(call_counter.load(Acquire), 1);
}

#[tokio::test]
async fn parallel_call_seq_await() {
    let call_counter = AtomicUsize::default();

    let group = Merger::<String, String, _>::new_per_process();
    let mut futures = Vec::new();
    for _ in 0..10 {
        futures.push(group.execute("key", || async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            call_counter.fetch_add(1, AcqRel);
            "Result".to_string()
        }));
    }

    for fut in futures {
        assert_eq!(fut.await, Ok("Result".to_string()));
    }
    assert_eq!(call_counter.load(Acquire), 1);
}

#[tokio::test]
async fn call_with_static_str_key() {
    let group = Merger::<String, String, _>::new_per_process();
    let result = group
        .execute("key", || async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            "Result".to_string()
        })
        .await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn call_with_static_string_key() {
    let group = Merger::<String, String, _>::new_per_process();
    let result = group
        .execute("key", || async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            "Result".to_string()
        })
        .await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn call_with_custom_key() {
    #[derive(Clone, PartialEq, Eq, Hash)]
    struct K(i32);
    let group = Merger::<K, String, _>::new_per_process();
    let result = group
        .execute(&K(1), || async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            "Result".to_string()
        })
        .await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn late_wait() {
    let group = Merger::<String, String, _>::new_per_process();
    let fut_early = group.execute("key", || async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        "Result".to_string()
    });
    let fut_late = group.execute("key", unreachable_future);
    assert_eq!(fut_early.await, Ok("Result".to_string()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fut_late.await, Ok("Result".to_string()));
}

#[tokio::test]
async fn cancel() {
    let group = Merger::<String, String, _>::new_per_process();

    // The executor was cancelled; the other awaiter will create a new future and execute.
    let fut_cancel = group.execute(&"key".to_string(), unreachable_future);
    let _ = tokio::time::timeout(Duration::from_millis(10), fut_cancel).await;
    let fut_late = group.execute("key", || async { "Result2".to_string() });
    assert_eq!(fut_late.await, Ok("Result2".to_string()));

    // the first executer is slow but not dropped, so the result will be the first ones.
    let begin = tokio::time::Instant::now();
    let fut_1 = group.execute("key", || async {
        tokio::time::sleep(Duration::from_millis(2000)).await;
        "Result1".to_string()
    });
    let fut_2 = group.execute(&"key".to_string(), unreachable_future);
    let (v1, v2) = tokio::join!(fut_1, fut_2);
    assert_eq!(v1, Ok("Result1".to_string()));
    assert_eq!(v2, Ok("Result1".to_string()));
    assert!(begin.elapsed() > Duration::from_millis(1500));
}

#[tokio::test]
async fn leader_panic_returns_error_to_all() {
    let group: Arc<Merger<String, String>> = Arc::new(Merger::new());

    // First task will panic (caught by catch_unwind)
    let group_clone = Arc::clone(&group);
    let leader_handle = tokio::spawn(async move {
        group_clone
            .execute("key", || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                panic!("leader panicked");
                #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
                "never".to_string()
            })
            .await
    });

    // Give time for the spawned task to register and start
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Second task joins as a follower
    let group_clone = Arc::clone(&group);
    let follower_handle = tokio::spawn(async move {
        group_clone
            .execute("key", || async {
                // This should never run - we're a follower
                "follower result".to_string()
            })
            .await
    });

    // Leader gets LeaderPanicked error (panic is caught, not propagated)
    let leader_result = leader_handle.await.expect("task should not panic - panic is caught");
    let Err(leader_err) = leader_result else {
        panic!("expected Err, got Ok");
    };
    assert_eq!(leader_err.message(), "leader panicked");

    // Follower also gets LeaderPanicked error with same message
    let follower_result = follower_handle.await.expect("follower task should not panic");
    let Err(follower_err) = follower_result else {
        panic!("expected Err, got Ok");
    };
    assert_eq!(follower_err.message(), "leader panicked");
}

#[tokio::test]
async fn debug_impl() {
    let group: Merger<String, String> = Merger::new();

    // Test Debug on empty group
    let debug_str = format!("{group:?}");
    assert!(debug_str.contains("Merger"));

    // Create a pending work item to populate the mapping
    let fut = group.execute("key", || async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        "Result".to_string()
    });

    // Debug should still work with entries in the mapping
    let debug_str = format!("{group:?}");
    assert!(debug_str.contains("Merger"));
    // The inner storage is a DashMap
    assert!(debug_str.contains("DashMap"));

    // Complete the work
    assert_eq!(fut.await, Ok("Result".to_string()));
}

#[tokio::test]
async fn per_process_strategy() {
    let group = Merger::<String, String, _>::new_per_process();
    let result = group.execute("key", || async { "Result".to_string() }).await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn per_numa_strategy() {
    let group = Merger::<String, String, _>::new_per_numa();
    let result = group.execute("key", || async { "Result".to_string() }).await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn per_core_strategy() {
    let group = Merger::<String, String, _>::new_per_core();
    let result = group.execute("key", || async { "Result".to_string() }).await;
    assert_eq!(result, Ok("Result".to_string()));
}

#[tokio::test]
async fn clone_shares_state() {
    let group1 = Merger::<String, String, _>::new_per_process();
    let group2 = group1.clone();

    let call_counter = AtomicUsize::default();

    // Start work on clone 1
    let fut1 = group1.execute("key", || async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        call_counter.fetch_add(1, AcqRel);
        "Result".to_string()
    });

    // Clone 2 should join the same work
    let fut2 = group2.execute("key", || async {
        call_counter.fetch_add(1, AcqRel);
        "Unreachable".to_string()
    });

    let (r1, r2) = tokio::join!(fut1, fut2);
    assert_eq!(r1, Ok("Result".to_string()));
    assert_eq!(r2, Ok("Result".to_string()));
    // Work should only execute once
    assert_eq!(call_counter.load(Acquire), 1);
}

#[tokio::test]
async fn leader_panicked_error_traits() {
    // Create an error by triggering a panic
    let group: Merger<String, String> = Merger::new();
    let result = group
        .execute("key", || async {
            panic!("test message");
            #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
            "never".to_string()
        })
        .await;
    let Err(error) = result else {
        panic!("expected Err");
    };

    // Test message()
    assert_eq!(error.message(), "test message");

    // Test Display - includes the panic message
    let display = format!("{error}");
    assert!(display.contains("leader task panicked"));
    assert!(display.contains("test message"));

    // Test Debug
    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("LeaderPanicked"));

    // Test Clone
    let cloned = error.clone();
    assert_eq!(cloned.message(), error.message());

    // Test PartialEq and Eq
    assert_eq!(error, cloned);

    // Test Error trait (can be used as a source error)
    let std_error: &dyn std::error::Error = &error;
    assert!(std_error.source().is_none());
}

#[tokio::test]
async fn retry_after_panic_succeeds() {
    let group: Merger<String, String> = Merger::new();

    // First call panics
    let result = group
        .execute("key", || async {
            panic!("intentional panic");
            #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
            "never".to_string()
        })
        .await;
    let Err(err) = result else {
        panic!("expected Err");
    };
    assert_eq!(err.message(), "intentional panic");

    // Retry with the same key should succeed
    let result = group.execute("key", || async { "success".to_string() }).await;
    assert_eq!(result, Ok("success".to_string()));
}

#[tokio::test]
async fn default_impl() {
    // Test that Default::default() works the same as new()
    let group1: Merger<String, String> = Merger::default();
    let group2: Merger<String, String> = Merger::new();

    let result1 = group1.execute("key", || async { "value".to_string() }).await;
    let result2 = group2.execute("key", || async { "value".to_string() }).await;

    assert_eq!(result1, Ok("value".to_string()));
    assert_eq!(result2, Ok("value".to_string()));
}

#[tokio::test]
async fn mixed_panic_and_success() {
    let group: Merger<String, String> = Merger::new();

    // Start multiple keys concurrently - some panic, some succeed
    let panic_fut = group.execute("panic_key", || async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        panic!("intentional panic");
        #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
        "never".to_string()
    });

    let success_fut = group.execute("success_key", || async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        "success".to_string()
    });

    let (panic_result, success_result) = tokio::join!(panic_fut, success_fut);

    // Panic key returns error with message
    let Err(err) = panic_result else {
        panic!("expected Err");
    };
    assert_eq!(err.message(), "intentional panic");

    // Success key returns value
    assert_eq!(success_result, Ok("success".to_string()));
}

#[tokio::test]
async fn follower_closure_not_called_on_panic() {
    let group: Arc<Merger<String, String>> = Arc::new(Merger::new());
    let follower_called = Arc::new(AtomicUsize::new(0));

    // Leader will panic
    let group_clone = Arc::clone(&group);
    let leader_handle = tokio::spawn(async move {
        group_clone
            .execute("key", || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                panic!("leader panic");
                #[expect(unreachable_code, reason = "Required to satisfy return type after panic")]
                "never".to_string()
            })
            .await
    });

    // Give leader time to start
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Follower joins - its closure should NOT be called
    let group_clone = Arc::clone(&group);
    let follower_called_clone = Arc::clone(&follower_called);
    let follower_handle = tokio::spawn(async move {
        group_clone
            .execute("key", || async {
                follower_called_clone.fetch_add(1, Acquire);
                "follower result".to_string()
            })
            .await
    });

    let (leader_result, follower_result) = tokio::join!(leader_handle, follower_handle);

    let Err(leader_err) = leader_result.expect("task join") else {
        panic!("expected Err");
    };
    assert_eq!(leader_err.message(), "leader panic");

    let Err(follower_err) = follower_result.expect("task join") else {
        panic!("expected Err");
    };
    assert_eq!(follower_err.message(), "leader panic");

    // Follower's closure was never called
    assert_eq!(follower_called.load(Acquire), 0);
}

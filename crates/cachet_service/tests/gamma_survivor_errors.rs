// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed adapter and extension failures through scripted in-memory services.

use std::future::ready;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use cachet_service::{CacheOperation, CacheResponse, CacheServiceExt, ServiceAdapter};
use cachet_tier::{CacheEntry, CacheTier, Error};
use layered::Service;

struct Scripted(bool);

#[cfg(test)]
impl Service<CacheOperation<String, i32>> for Scripted {
    type Out = Result<CacheResponse<i32>, Error>;

    fn execute(&self, input: CacheOperation<String, i32>) -> impl Future<Output = Self::Out> + Send {
        ready(if self.0 {
            Err(Error::from_message("scripted failure"))
        } else {
            Ok(match input {
                CacheOperation::Clear => CacheResponse::Get(None),
                _ => CacheResponse::Clear,
            })
        })
    }
}

#[cfg(test)]
fn completed<T>(future: impl Future<Output = T>) -> T {
    let mut future = pin!(future);
    let Poll::Ready(value) = future.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("the scripted service is immediately ready");
    };
    value
}

#[test]
fn gamma_survivor_adapter_propagates_service_errors_and_names_wrong_operations() {
    let key = String::from("key");
    for fail in [false, true] {
        let adapter = ServiceAdapter::new(Scripted(fail));
        let errors = [
            completed(adapter.get(&key)).unwrap_err(),
            completed(adapter.insert(key.clone(), CacheEntry::new(1))).unwrap_err(),
            completed(adapter.invalidate(&key)).unwrap_err(),
            completed(adapter.clear()).unwrap_err(),
        ];
        for (error, operation) in errors.into_iter().zip(["get", "insert", "invalidate", "clear"]) {
            let expected = if fail {
                String::from("scripted failure")
            } else {
                format!("unexpected response type for {operation}")
            };
            assert_eq!(error.to_string(), expected);
        }
    }
}

#[test]
fn gamma_survivor_extensions_propagate_service_errors_and_wrong_response_diagnostics() {
    let key = String::from("key");
    for fail in [false, true] {
        let service = Scripted(fail);
        let errors = [
            completed(CacheServiceExt::get(&service, &key)).unwrap_err(),
            completed(CacheServiceExt::insert(&service, key.clone(), CacheEntry::new(1))).unwrap_err(),
            completed(CacheServiceExt::invalidate(&service, &key)).unwrap_err(),
            completed(CacheServiceExt::clear(&service)).unwrap_err(),
        ];
        for error in errors {
            assert_eq!(
                error.to_string(),
                if fail { "scripted failure" } else { "unexpected response type" }
            );
        }
    }
}

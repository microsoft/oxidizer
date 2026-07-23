// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation contracts for generated query codecs.

#![cfg(not(miri))]

use alloc_tracker::{Allocator, Session};
use routerama::query::{FromQuery, ToQuery};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn total_bytes_allocated(session: &Session, operation_name: &str) -> u64 {
    let missing_operation = format!("operation {operation_name:?} must match a name registered with Session::operation on this session");

    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == operation_name).then(|| operation.total_bytes_allocated()))
        .expect(&missing_operation)
}

#[derive(routerama::query::FromQuery, routerama::query::ToQuery)]
struct CommonQuery<'q> {
    q: &'q str,
    page: u32,
    exact: bool,
}

#[test]
fn common_parse_is_allocation_free() {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("query_parse");
    {
        let _span = operation.measure_thread().iterations(1);
        let parsed = CommonQuery::from_query("q=rust&page=2&exact=true").expect("valid query");
        std::hint::black_box(parsed);
    }
    assert_eq!(total_bytes_allocated(&session, "query_parse"), 0);
}

#[test]
fn production_into_reserved_string_is_allocation_free() {
    let query = CommonQuery {
        q: "rust",
        page: 2,
        exact: true,
    };
    let mut output = String::with_capacity(64);
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("query_produce");
    {
        let _span = operation.measure_thread().iterations(1);
        query.write_query(&mut output).expect("query production succeeds");
    }
    assert_eq!(total_bytes_allocated(&session, "query_produce"), 0);
    assert_eq!(output, "q=rust&page=2&exact=true");
}

#[derive(Debug, routerama::query::FromQuery)]
struct BorrowedQuery<'q> {
    q: &'q str,
}

#[test]
fn a_borrowed_field_rejects_an_encoded_value_without_decoding_it() {
    let borrowed = BorrowedQuery::from_query("q=rust").expect("an unencoded value borrows");
    assert_eq!(borrowed.q, "rust");

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("borrow_required");
    let error = {
        let _span = operation.measure_thread().iterations(1);
        BorrowedQuery::from_query("q=rust+lang%2Fnow").expect_err("a borrowed field cannot own decoded data")
    };

    assert_eq!(error.parameter(), Some("q"));
    assert_eq!(error.kind(), routerama::query::ErrorKind::BorrowRequired);
    assert_eq!(total_bytes_allocated(&session, "borrow_required"), 0);
}

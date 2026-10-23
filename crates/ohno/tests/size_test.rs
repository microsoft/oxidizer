// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use std::mem;

use ohno::OhnoCore;

#[test]
fn test_inner_error_size() {
    // OhnoCore should now be just a pointer to boxed data
    // On 64-bit systems, this should be 8 bytes (just a Box pointer)
    let size = mem::size_of::<OhnoCore>();
    println!("OhnoCore size: {size} bytes");

    // It should be much smaller than before (which would have included
    // the full Backtrace, Vec<EnrichmentEntry>, and Source inline)
    // On 64-bit systems, expect 8 bytes for the Box pointer
    assert_eq!(size, mem::size_of::<usize>());
}

#[test]
fn test_result_with_inner_error_is_reasonable_size() {
    // This should no longer trigger clippy::result_large_err
    let size = mem::size_of::<Result<String, OhnoCore>>();
    println!("Result<String, OhnoCore> size: {size} bytes");

    // Should be reasonable sized now - typically the discriminant + max of the variants
    // On 64-bit: 8 bytes for the discriminant + max(24 bytes for String, 8 bytes for OhnoCore)
    // So around 32 bytes total
    assert!(size <= 32, "Result size should be reasonable: {size} bytes");
}

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared helper for printing the `OTel` [`Resource`] header exactly once.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;

use crate::format::print_resource;

/// Tracks a [`Resource`] and prints it exactly once on the first call to
/// [`ResourceHeader::print_once`].
///
/// Uses an [`AtomicBool`] internally so `print_once` only requires `&self`.
#[derive(Debug)]
pub struct ResourceHeader {
    resource: Resource,
    emitted: AtomicBool,
}

impl ResourceHeader {
    /// Creates a header for the empty default resource.
    #[must_use]
    pub fn new() -> Self {
        Self {
            resource: Resource::builder().build(),
            emitted: AtomicBool::new(false),
        }
    }

    /// Updates the stored resource and resets the print flag.
    ///
    /// Requires `&mut self` (called from `LogExporter::set_resource(&mut self)`).
    pub fn set_resource(&mut self, res: &Resource) {
        self.resource = res.clone();
        *self.emitted.get_mut() = false;
    }

    /// Prints the resource header followed by a newline if it hasn't been printed yet.
    ///
    /// # Errors
    ///
    /// Returns an [`opentelemetry_sdk::error::OTelSdkError::InternalFailure`]
    /// with the reason for the underlying failure, if writing to the provided writer fails.
    pub fn print_once(&self, w: &mut (impl Write + ?Sized)) -> OTelSdkResult {
        if self
            .emitted
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let result = (|| {
                print_resource(w, &self.resource)?;
                writeln!(w).map_err(|e| opentelemetry_sdk::error::OTelSdkError::InternalFailure(e.to_string()))
            })();
            if result.is_err() {
                self.emitted.store(false, Ordering::SeqCst);
            }
            result?;
        }
        Ok(())
    }
}

impl Default for ResourceHeader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Use one writer interface so LLVM measures success and failure in the same generic instance.
    //! Tests other than the default-constructor smoke test use detector-free resource fixtures
    //! to exercise the real header without querying the host executable.

    use opentelemetry::KeyValue;
    use opentelemetry_sdk::error::OTelSdkError;

    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Default resource detection calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn header_is_printed_once() {
        let header = ResourceHeader::default();
        let mut output = Vec::new();

        header.print_once(&mut output as &mut (dyn Write + Send)).unwrap();

        assert!(!output.is_empty());

        output.clear();

        header.print_once(&mut output as &mut (dyn Write + Send)).unwrap();

        assert!(output.is_empty());
    }

    #[test]
    fn changing_resource_prints_new_header() {
        let mut header = ResourceHeader {
            resource: Resource::builder_empty().build(),
            emitted: AtomicBool::new(false),
        };
        let mut output = Vec::new();
        let resource = Resource::builder_empty()
            .with_schema_url([KeyValue::new("service.name", "example")], "https://example.test/schema")
            .build();

        header.print_once(&mut output as &mut (dyn Write + Send)).unwrap();
        output.clear();

        header.set_resource(&resource);
        header.print_once(&mut output as &mut (dyn Write + Send)).unwrap();

        assert_eq!(output.as_slice(), b"<Resource: https://example.test/schema service.name=example>\n");
    }

    #[test]
    fn header_write_failure_is_reported_as_sdk_error() {
        let header = ResourceHeader {
            resource: Resource::builder_empty().build(),
            emitted: AtomicBool::new(false),
        };

        let err = header.print_once(&mut [].as_mut_slice() as &mut (dyn Write + Send)).unwrap_err();

        assert!(matches!(err, OTelSdkError::InternalFailure(_)));
    }

    #[test]
    fn newline_write_failure_is_reported_as_sdk_error() {
        let header = ResourceHeader {
            resource: Resource::builder_empty().build(),
            emitted: AtomicBool::new(false),
        };

        // Leave room for the empty resource header, but not its trailing newline.
        let mut buffer = [0u8; b"<Resource: >".len()];

        let err = header
            .print_once(&mut buffer.as_mut_slice() as &mut (dyn Write + Send))
            .unwrap_err();

        assert!(matches!(err, OTelSdkError::InternalFailure(_)));
    }
}

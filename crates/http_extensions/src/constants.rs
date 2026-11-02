// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The default response buffer limit is 2 GB.
///
/// This value follows the .NET default:
///
/// <https://learn.microsoft.com/en-us/dotnet/api/system.net.http.httpclient.maxresponsecontentbuffersize>
///
/// A future release can reduce this large limit.
pub(crate) const DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES: usize = 2 * 1024 * 1024 * 1024;

#[cfg(any(feature = "test-util", test))]
pub(crate) const ERR_POISONED_LOCK: &str =
    "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn default_response_buffer_limit_bytes_ok() {
        assert_eq!(DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES, 2_147_483_648);
    }
}

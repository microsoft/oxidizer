// Copyright (c) Microsoft Corporation.

/// Default response buffer limit of 2GB. This follows the defaults in .NET:
///
/// <https://learn.microsoft.com/en-us/dotnet/api/system.net.http.httpclient.maxresponsecontentbuffersize>
///
/// Maybe this is too excessive and should be reduced in the future, but for now it is set to 2GB.
pub(crate) const DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES: usize = 2 * 1024 * 1024 * 1024;

#[cfg(any(feature = "test-util", test))]
pub(crate) const ERR_POISONED_LOCK: &str =
    "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_response_buffer_limit_bytes_ok() {
        assert_eq!(DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES, 2_147_483_648);
    }
}

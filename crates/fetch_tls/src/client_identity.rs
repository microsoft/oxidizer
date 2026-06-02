// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Backend-agnostic `mTLS` client identity.
//!
//! [`ClientIdentity`] holds a `DER`-encoded certificate chain and private key
//! and is consumed by both backends, so `mTLS` is configured the same way
//! regardless of which backend is selected.

use std::fmt;

use rustls_pki_types::pem::PemObject;

/// Error returned when constructing a [`ClientIdentity`] from key material fails.
#[ohno::error]
pub struct ClientIdentityError;

/// Client identity for mutual TLS (`mTLS`) authentication.
///
/// Holds the client certificate chain and private key presented during the
/// TLS handshake. The same value works with either backend; each one
/// converts the contained `DER` bytes into its own internal form.
///
/// # Example
///
/// ```rust,no_run
/// use fetch_tls::ClientIdentity;
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let cert_pem = std::fs::read("client.pem")?;
/// let key_pem = std::fs::read("client-key.pem")?;
/// let identity = ClientIdentity::from_pem(&cert_pem, &key_pem)?;
/// # let _ = identity;
/// # Ok(())
/// # }
/// ```
pub struct ClientIdentity {
    cert_chain: Vec<rustls_pki_types::CertificateDer<'static>>,
    private_key: rustls_pki_types::PrivateKeyDer<'static>,
}

impl Clone for ClientIdentity {
    fn clone(&self) -> Self {
        Self {
            cert_chain: self.cert_chain.clone(),
            // `PrivateKeyDer` does not implement `Clone` because the inner key
            // material is sensitive; `clone_key` makes the copy explicit.
            private_key: self.private_key.clone_key(),
        }
    }
}

impl ClientIdentity {
    /// Creates a client identity from `PEM`-encoded certificate and private key.
    ///
    /// `cert_pem` may contain one or more certificates (leaf first, then
    /// intermediates). `key_pem` must contain exactly one private key
    /// (`PKCS#1`, `PKCS#8`, or `SEC1`).
    ///
    /// The native-tls backend only accepts `PKCS#8` keys; `PKCS#1` and
    /// `SEC1` work with rustls but cause native-tls to fail at build time.
    ///
    /// # Errors
    ///
    /// Returns an error if either input is not valid `PEM`.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, ClientIdentityError> {
        let cert_chain: Vec<rustls_pki_types::CertificateDer<'static>> = rustls_pki_types::CertificateDer::pem_slice_iter(cert_pem)
            .collect::<Result<_, _>>()
            .map_err(ClientIdentityError::caused_by)?;
        let private_key = rustls_pki_types::PrivateKeyDer::from_pem_slice(key_pem).map_err(ClientIdentityError::caused_by)?;
        Ok(Self { cert_chain, private_key })
    }

    /// Creates a client identity from `DER`-encoded certificate and private key.
    ///
    /// `cert_chain` is leaf-first; `key_der` must be a `PKCS#8`-encoded
    /// private key.
    #[must_use]
    pub fn from_der<I, C, K>(cert_chain: I, key_der: K) -> Self
    where
        I: IntoIterator<Item = C>,
        C: AsRef<[u8]>,
        K: AsRef<[u8]>,
    {
        let cert_chain = cert_chain
            .into_iter()
            .map(|c| rustls_pki_types::CertificateDer::from(c.as_ref().to_vec()))
            .collect();
        let private_key = rustls_pki_types::PrivateKeyDer::from(rustls_pki_types::PrivatePkcs8KeyDer::from(key_der.as_ref().to_vec()));
        Self { cert_chain, private_key }
    }

    /// Returns the certificate chain as rustls types.
    #[cfg(any(feature = "rustls", test))]
    #[cfg_attr(test, mutants::skip)] // trivial accessor, tested via `mTLS` integration
    pub(crate) fn cert_chain(&self) -> &[rustls_pki_types::CertificateDer<'static>] {
        &self.cert_chain
    }

    /// Returns the private key as rustls types.
    #[cfg(any(feature = "rustls", test))]
    pub(crate) fn private_key(&self) -> &rustls_pki_types::PrivateKeyDer<'static> {
        &self.private_key
    }

    /// Builds a [`native_tls::Identity`] from this client identity.
    ///
    /// Re-encodes the `DER` components as `PEM` and feeds them to
    /// [`native_tls::Identity::from_pkcs8`], the format supported across all
    /// platform backends. Fails if the private key is not `PKCS#8` or if the
    /// platform native TLS implementation rejects the material.
    #[cfg(any(feature = "native-tls", test))]
    pub(crate) fn build_native_identity(&self) -> Result<native_tls::Identity, ClientIdentityError> {
        let key_pkcs8_der = match &self.private_key {
            rustls_pki_types::PrivateKeyDer::Pkcs8(key) => key.secret_pkcs8_der(),
            rustls_pki_types::PrivateKeyDer::Pkcs1(_) | rustls_pki_types::PrivateKeyDer::Sec1(_) => {
                return Err(ClientIdentityError::caused_by(
                    "native-tls backend requires a `PKCS#8` private key (got `PKCS#1` or `SEC1`)",
                ));
            }
            _ => {
                return Err(ClientIdentityError::caused_by("native-tls backend requires a `PKCS#8` private key"));
            }
        };

        let mut cert_pem = Vec::new();
        for cert in &self.cert_chain {
            write_pem_block(&mut cert_pem, "CERTIFICATE", cert.as_ref());
        }
        let mut key_pem = Vec::new();
        write_pem_block(&mut key_pem, "PRIVATE KEY", key_pkcs8_der);

        native_tls::Identity::from_pkcs8(&cert_pem, &key_pem).map_err(ClientIdentityError::caused_by)
    }
}

/// Writes a `PEM`-encoded object to `out`.
///
/// Format per `RFC 7468`: a textual `-----BEGIN <label>-----` line, the body
/// as `base64` broken into 64-character lines, and a matching
/// `-----END <label>-----` line.
#[cfg(any(feature = "native-tls", test))]
fn write_pem_block(out: &mut Vec<u8>, label: &str, der: &[u8]) {
    use std::io::Write;

    use base64::Engine as _;

    let body = base64::engine::general_purpose::STANDARD.encode(der);
    writeln!(out, "-----BEGIN {label}-----").expect("writing to Vec cannot fail");
    for line in body.as_bytes().chunks(64) {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    writeln!(out, "-----END {label}-----").expect("writing to Vec cannot fail");
}

impl fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("cert_chain_len", &self.cert_chain.len())
            .field("private_key", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivateSec1KeyDer};

    use super::*;

    #[test]
    fn from_pem_parses_valid_cert_and_key() {
        // `rustls_pki_types`' PEM parser identifies items by label and treats
        // the body as opaque DER, so short placeholder bytes round-trip fine
        // for the purpose of exercising the happy path.
        let mut cert_pem = Vec::new();
        write_pem_block(&mut cert_pem, "CERTIFICATE", &[0x30, 0x00]);
        write_pem_block(&mut cert_pem, "CERTIFICATE", &[0x30, 0x01, 0x00]);

        let mut key_pem = Vec::new();
        write_pem_block(&mut key_pem, "PRIVATE KEY", &[0x30, 0x00]);

        let identity = ClientIdentity::from_pem(&cert_pem, &key_pem).expect("valid PEM should parse");
        assert_eq!(identity.cert_chain().len(), 2);
        assert!(matches!(identity.private_key(), rustls_pki_types::PrivateKeyDer::Pkcs8(_)));
    }

    #[test]
    fn from_pem_fails_for_invalid_pem() {
        ClientIdentity::from_pem(b"not a pem", b"not a key").unwrap_err();
    }

    #[test]
    fn from_der_constructs_identity() {
        let identity = ClientIdentity::from_der(vec![vec![0x30u8, 0x00]], vec![0x30u8, 0x00]);
        assert_eq!(identity.cert_chain.len(), 1);
    }

    #[test]
    fn clone_preserves_cert_chain_length() {
        let identity = ClientIdentity::from_der(vec![vec![0x30u8, 0x00], vec![0x30u8, 0x00]], vec![0x30u8, 0x00]);
        let cloned = identity.clone();
        assert_eq!(identity.cert_chain.len(), 2);
        assert_eq!(cloned.cert_chain.len(), 2);
    }

    #[test]
    fn debug_redacts_private_key() {
        let identity = ClientIdentity::from_der(vec![vec![0x30u8, 0x00]], vec![0x30u8, 0x00]);
        let debug_output = format!("{identity:?}");
        assert!(debug_output.contains("<redacted>"));
        assert!(debug_output.contains("cert_chain_len: 1"));
    }

    #[test]
    // Miri can't execute the platform TLS FFI (`CertOpenStore` on Windows, etc.)
    // that `native_tls::Identity::from_pkcs8` invokes to validate the inputs.
    #[cfg_attr(miri, ignore)]
    fn build_native_identity_fails_for_invalid_certificate() {
        let identity = ClientIdentity::from_der(vec![vec![0xffu8, 0xff]], vec![0x30u8, 0x00]);

        let Err(err) = identity.build_native_identity() else {
            panic!("expected error for invalid certificate");
        };
        // The PKCS#8 key must be accepted by the match (the failure should come
        // from `native_tls` rejecting the bogus certificate, not from our own
        // "requires a `PKCS#8` private key" guard).
        let msg = format!("{err}");
        assert!(
            !msg.contains("requires a `PKCS#8` private key"),
            "PKCS#8 key was incorrectly rejected: {msg}"
        );
    }

    #[test]
    fn build_native_identity_rejects_pkcs1_key() {
        // Construct directly so the private key is the `PKCS#1` variant rather
        // than the `PKCS#8` wrapper that `from_der` produces.
        let identity = ClientIdentity {
            cert_chain: vec![CertificateDer::from(vec![0x30, 0x00])],
            private_key: PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(vec![0x30, 0x00])),
        };

        let Err(err) = identity.build_native_identity() else {
            panic!("expected error for PKCS#1 key");
        };
        assert!(format!("{err}").contains("`PKCS#1` or `SEC1`"));
    }

    #[test]
    fn build_native_identity_rejects_sec1_key() {
        let identity = ClientIdentity {
            cert_chain: vec![CertificateDer::from(vec![0x30, 0x00])],
            private_key: PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(vec![0x30, 0x00])),
        };

        let Err(err) = identity.build_native_identity() else {
            panic!("expected error for SEC1 key");
        };
        assert!(format!("{err}").contains("`PKCS#1` or `SEC1`"));
    }

    #[test]
    fn write_pem_block_round_trips() {
        let mut out = Vec::new();
        write_pem_block(&mut out, "PRIVATE KEY", b"hello");
        let text = std::str::from_utf8(&out).unwrap();
        assert!(text.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        assert!(text.contains("aGVsbG8=")); // base64("hello")
        assert!(text.trim_end().ends_with("-----END PRIVATE KEY-----"));
    }
}

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private native builder recording and deterministic build-failure injection.

use std::cell::RefCell;

use native_tls::{Certificate, Error, Identity, Protocol, TlsConnector, TlsConnectorBuilder};

#[derive(Default)]
pub(super) struct Recording {
    pub(super) roots: Vec<Vec<u8>>,
    pub(super) build_calls: usize,
    reject_build: bool,
}

thread_local! {
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

pub(super) fn record<T>(reject_build: bool, action: impl FnOnce() -> T) -> (T, Recording) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RECORDING.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    RECORDING.with(|slot| {
        assert!(
            slot.borrow_mut()
                .replace(Recording {
                    reject_build,
                    ..Recording::default()
                })
                .is_none()
        );
    });
    let _reset = Reset;
    let value = action();
    let recording = RECORDING.with(|slot| slot.borrow_mut().take().unwrap());
    (value, recording)
}

pub(super) struct Builder(TlsConnectorBuilder);

impl Builder {
    pub(super) fn new() -> Self {
        Self(TlsConnector::builder())
    }

    pub(super) fn request_alpns(&mut self, protocols: &[&str]) -> &mut Self {
        self.0.request_alpns(protocols);
        self
    }

    pub(super) fn min_protocol_version(&mut self, protocol: Option<Protocol>) -> &mut Self {
        self.0.min_protocol_version(protocol);
        self
    }

    pub(super) fn add_root_certificate(&mut self, certificate: Certificate) -> &mut Self {
        RECORDING.with(|slot| {
            if let Some(recording) = slot.borrow_mut().as_mut() {
                recording.roots.push(certificate.to_der().unwrap());
            }
        });
        self.0.add_root_certificate(certificate);
        self
    }

    pub(super) fn identity(&mut self, identity: Identity) -> &mut Self {
        self.0.identity(identity);
        self
    }

    pub(super) fn build(&self) -> Result<TlsConnector, Error> {
        let reject = RECORDING.with(|slot| {
            slot.borrow_mut().as_mut().is_some_and(|recording| {
                recording.build_calls += 1;
                recording.reject_build
            })
        });
        if reject {
            return Err(Certificate::from_der(&[]).err().unwrap());
        }
        self.0.build()
    }
}

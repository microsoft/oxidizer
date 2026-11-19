// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private configuration recorder; hyper's `HttpConnector` Debug omits its options.

use std::cell::Cell;
use std::ops::{Deref, DerefMut};

use fetch_options::SocketOptions;
use hyper_util::client::legacy::connect::HttpConnector as NativeConnector;

thread_local! {
    static OBSERVED: Cell<Option<SocketOptions>> = const { Cell::new(None) };
}

#[derive(Clone, Debug)]
pub(super) struct HttpConnector {
    inner: NativeConnector,
    socket: SocketOptions,
}

impl HttpConnector {
    pub(super) fn new() -> Self {
        OBSERVED.set(Some(SocketOptions::default()));
        Self {
            inner: NativeConnector::new(),
            socket: SocketOptions::default(),
        }
    }

    pub(super) fn set_nodelay(&mut self, value: bool) {
        self.socket.no_delay = Some(value);
        OBSERVED.set(Some(self.socket));
        self.inner.set_nodelay(value);
    }

    pub(super) fn set_send_buffer_size(&mut self, value: Option<usize>) {
        self.socket.send_buffer_size = value.map(|value| u32::try_from(value).unwrap());
        OBSERVED.set(Some(self.socket));
        self.inner.set_send_buffer_size(value);
    }

    pub(super) fn set_recv_buffer_size(&mut self, value: Option<usize>) {
        self.socket.receive_buffer_size = value.map(|value| u32::try_from(value).unwrap());
        OBSERVED.set(Some(self.socket));
        self.inner.set_recv_buffer_size(value);
    }
}

impl Deref for HttpConnector {
    type Target = NativeConnector;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for HttpConnector {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub(super) fn observed() -> Option<SocketOptions> {
    OBSERVED.get()
}

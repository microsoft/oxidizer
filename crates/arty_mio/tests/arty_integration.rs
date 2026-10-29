// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration between the public Arty context path and Mio readiness.

#![allow(clippy::unwrap_used, reason = "test code")]

testing_aids::init_tracing!();

use std::io;
use std::net::UdpSocket as StdUdpSocket;

use arty::task::Builtins;
use arty_mio::{MioContext, MioOptions};
use mio::Interest;
use mio::net::UdpSocket;

#[arty::test]
async fn builtins_context_drives_a_mio_source(builtins: Builtins) {
    let io = builtins.io().context::<MioContext>(&MioOptions::default());
    let receiver = UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let sender = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();
    let mut registration = io.register(receiver, Interest::READABLE).unwrap();

    sender.send(&[1]).unwrap();
    std::future::poll_fn(|context| registration.poll_read_ready(context)).await.unwrap();

    let mut buffer = [0; 8];
    assert_eq!(registration.source_mut().unwrap().recv(&mut buffer).unwrap(), 1);
    assert_eq!(
        registration.source_mut().unwrap().recv(&mut buffer).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

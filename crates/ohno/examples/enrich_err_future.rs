// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::Poll;

use futures::task::{Context, noop_waker};
use ohno::enrich_err;

#[ohno::error]
struct MyError;

#[derive(Default)]
struct SimpleFuture {
    poll_count: u32,
}

impl Future for SimpleFuture {
    type Output = Result<String, MyError>;

    #[enrich_err("SimpleFuture::poll failed after {} polls", self.poll_count)]
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll_count += 1;
        match self.poll_count {
            1 => Poll::Pending,
            2 => Poll::Ready(Ok("Success!".to_string())),
            _ => Poll::Ready(Err(MyError::caused_by("poll after ready"))),
        }
    }
}

fn poll_once<F>(future: &mut Pin<&mut F>) -> Poll<F::Output>
where
    F: Future,
{
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    future.as_mut().poll(&mut cx)
}

fn main() {
    let mut future = SimpleFuture::default();
    let mut pinned = Pin::new(&mut future);

    for i in 1..6 {
        println!("poll #{i}:");
        match poll_once(&mut pinned) {
            Poll::Ready(Ok(s)) => println!("  Ok: {s}"),
            Poll::Ready(Err(e)) => println!("  Err: {e}"),
            Poll::Pending => println!("  Pending"),
        }
    }
}

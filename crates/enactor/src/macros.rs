// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Sends an ask message and waits for the reply.
///
/// Use [`ActorRef::ask`](crate::ActorRef::ask) when the ask variant takes only
/// [`Reply<T>`](crate::Reply). Use `ask!` when the ask variant also carries
/// payload and the trailing reply field needs to be appended.
///
/// # Examples
///
/// ```rust
/// # use enactor::{ActorRef, AskError, Reply};
/// # type UserId = u64;
/// # #[derive(Debug, PartialEq, Eq)] struct User;
/// # enum Message { Lookup(UserId, Reply<User>) }
/// # async fn example(actor: ActorRef<Message>, id: UserId) -> Result<User, AskError> {
///    let user = enactor::ask!(actor, Message::Lookup(id)).await?;
/// # Ok(user)
/// # }
/// ```
#[macro_export]
macro_rules! ask {
    ($actor:expr, $($variant:ident)::+ ( $($args:expr),* $(,)? ) $(,)?) => {{
        let actor = ::core::clone::Clone::clone(&$actor);
        async move {
            let (reply, rx) = $crate::Reply::channel();
            actor.tell($($variant)::+($($args,)* reply)).await?;
            rx.await.map_err($crate::AskError::from)
        }
    }};
    ($actor:expr, $variant:path $(,)?) => {
        ($actor).ask($variant)
    };
}

/// Sends an ask message and waits up to a timeout for the reply.
///
/// `ask_t!` takes the same message forms as [`ask!`](macro@crate::ask), then a
/// [`&Clock`](tick::Clock) and a [`Duration`](std::time::Duration). One
/// deadline covers the wait for mailbox capacity and the wait for the reply.
/// [`AskError::is_timeout`](crate::AskError::is_timeout) reports a missed
/// deadline.
///
/// A request that timed out can still be handled. If the mailbox accepted the
/// request before the deadline, the actor still handles it and its reply is
/// dropped.
///
/// # Examples
///
/// ```rust
/// # use std::time::Duration;
/// # use enactor::{ActorRef, AskError, Reply};
/// # use tick::Clock;
/// # type UserId = u64;
/// # #[derive(Debug, PartialEq, Eq)] struct User;
/// # enum Message { Lookup(UserId, Reply<User>) }
/// # async fn example(actor: ActorRef<Message>, id: UserId, clock: &Clock) -> Result<User, AskError> {
/// let user = enactor::ask_t!(actor, Message::Lookup(id), clock, Duration::from_secs(1)).await?;
/// # Ok(user)
/// # }
/// ```
#[macro_export]
macro_rules! ask_t {
    ($actor:expr, $($variant:ident)::+ ( $($args:expr),* $(,)? ), $clock:expr, $timeout:expr $(,)?) => {{
        let actor = ::core::clone::Clone::clone(&$actor);
        let clock = ::core::clone::Clone::clone($clock);
        let timeout = $timeout;
        async move {
            let (reply, rx) = $crate::Reply::channel();
            actor
                .ask_message_timeout($($variant)::+($($args,)* reply), rx, &clock, timeout)
                .await
        }
    }};
    ($actor:expr, $variant:path, $clock:expr, $timeout:expr $(,)?) => {
        ($actor).ask_timeout($variant, $clock, $timeout)
    };
}

/// Sends a message expression with [`ActorRef::tell`](crate::ActorRef::tell).
///
/// # Examples
///
/// ```rust
/// # use enactor::{ActorRef, TellError};
/// # enum Message { Tick }
/// # async fn example(actor: ActorRef<Message>) -> Result<(), TellError> {
/// enactor::tell!(actor, Message::Tick).await?;
/// # Ok(())
/// # }
/// ```
#[macro_export]
macro_rules! tell {
    ($actor:expr, $message:expr $(,)?) => {
        ($actor).tell($message)
    };
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use crate::Reply;
    use crate::actor::{ActorId, ActorRef};
    use crate::inbox::{Channel, Control};

    enum Message {
        Lookup(u8, Reply<u8>),
    }

    #[test]
    fn ask_macro_accepts_constructor_syntax() {
        let (message_tx, message_rx) = Channel::<Message>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(ActorId::next(), message_tx, control_tx);

        let result = block_on(async {
            let receive = async {
                let message = message_rx.recv().await.expect("message should arrive");
                let Message::Lookup(value, reply) = message;
                reply.send(value + 1).expect("reply should be received");
            };

            let call = crate::ask!(actor_ref, Message::Lookup(41));
            let ((), result) = futures::future::join(receive, call).await;
            result
        });

        assert_eq!(result.expect("ask should succeed"), 42);
    }
}

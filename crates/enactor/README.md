<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Enactor Logo" width="96">

# Enactor

[![crate.io](https://img.shields.io/crates/v/enactor.svg)](https://crates.io/crates/enactor)
[![docs.rs](https://docs.rs/enactor/badge.svg)](https://docs.rs/enactor)
[![MSRV](https://img.shields.io/crates/msrv/enactor)](https://crates.io/crates/enactor)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Typed actors for the [Arty][__link0] thread-per-core runtime.

An actor keeps its own state and handles one message at a time. Each actor
runs as one Arty task, so it stays on one worker thread for its whole life.
Use [`ActorRuntime`][__link1] to spawn an actor, [`ActorRef`][__link2] to send it work, and
[`Context`][__link3] inside [`Actor::handle`][__link4] for self-sends and lifecycle requests.

Each actor has two queues. Messages use a bounded mailbox. Stop and drain
requests use a separate control queue. The actor reads the control queue
before the next message, so a full mailbox does not delay a stop.

Message types are usually enums. Tell-style variants carry only their
payload. Ask-style variants end with [`Reply<T>`][__link5], so the actor can
send one value back to the caller.

## Getting started

```rust
use enactor::{Actor, ActorRef, ActorRuntime, Context, Reply};

enum Message {
    Add(u64),
    Total(Reply<u64>),
}

struct Counter {
    total: u64,
}

impl Actor for Counter {
    type Message = Message;

    async fn handle(&mut self, message: Message, _ctx: &mut Context<Message>) {
        match message {
            Message::Add(value) => self.total += value,
            Message::Total(reply) => {
                let _ = reply.send(self.total);
            }
        }
    }
}

#[arty::main]
async fn main(cx: arty::task::Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = ActorRuntime::from_builtins(&cx);
    let counter: ActorRef<Message> = runtime.spawn(Counter { total: 0 });

    counter.tell(Message::Add(2)).await?;
    enactor::tell!(counter, Message::Add(3)).await?;
    assert_eq!(counter.ask(Message::Total).await?, 5);

    counter.stop().await?;
    Ok(())
}
```

Use [`ActorRuntime::spawn`][__link6] for current-thread placement,
[`ActorRuntime::spawn_anywhere`][__link7] for runtime-selected placement, or
[`ActorRuntime::builder`][__link8] for explicit placement, inbox capacity, or a
stream-backed actor.

Send fire-and-forget work with [`ActorRef::tell`][__link9] or [`tell!`][__link10].
Use [`ActorRef::tell_owned`][__link11] or [`ActorRef::try_tell`][__link12] when a rejected
message must be returned to the caller.
Request a value back with [`ActorRef::ask`][__link13] for zero-payload ask variants.
[`ask!`][__link14] provides constructor-call syntax when the ask variant
also carries caller-provided payload.

The `examples` folder contains `enactor_ping_pong`, `enactor_counter`, and
`enactor_worker_pool`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/enactor">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbpLCmO2CPek4bM3O9OSHTpK0bft_aHuzMflMbUZ7SENFN82xhZIKCZGFydHllMC42LjGCZ2VuYWN0b3JlMC4xLjA
 [__link0]: https://crates.io/crates/arty/0.6.1
 [__link1]: https://docs.rs/enactor/0.1.0/enactor/?search=ActorRuntime
 [__link10]: macro@tell
 [__link11]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::ActorRef::tell_owned
 [__link12]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::ActorRef::try_tell
 [__link13]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::ActorRef::ask
 [__link14]: macro@ask
 [__link2]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::ActorRef
 [__link3]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::Context
 [__link4]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::Actor::handle
 [__link5]: https://docs.rs/enactor/0.1.0/enactor/?search=Reply
 [__link6]: https://docs.rs/enactor/0.1.0/enactor/?search=ActorRuntime::spawn
 [__link7]: https://docs.rs/enactor/0.1.0/enactor/?search=ActorRuntime::spawn_anywhere
 [__link8]: https://docs.rs/enactor/0.1.0/enactor/?search=ActorRuntime::builder
 [__link9]: https://docs.rs/enactor/0.1.0/enactor/?search=actor::ActorRef::tell

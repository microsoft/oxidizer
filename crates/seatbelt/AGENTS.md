# AI Agents Guidelines for `seatbelt`

Code in this crate should follow the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## Dual Service Implementations

Each middleware has two logic-equivalent `Service` trait impls in the same file: one for
`layered::Service` (`execute`) and one for `tower_service::Service` (`call`). The business
logic flow must be identical between the two — they differ only mechanically (boxed futures,
`poll_ready`, `Arc` cloning, `Result` output type).

**Any change to `execute` must be mirrored in `call`, and vice versa.** Prefer adding logic to
the shared `*Shared` helper structs rather than inlining into both methods. Integration tests
use `rstest` cases that exercise both implementations — new test scenarios should do the same.
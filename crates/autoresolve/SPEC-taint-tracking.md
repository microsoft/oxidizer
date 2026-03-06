# Taint-tracking for scoped resolver sharing

## Problem

When sibling scoped resolvers resolve the same type that only depends on
shared (ancestor) data, each child constructs its own copy independently.
The type should be resolved once and stored in the shared ancestor so all
siblings reuse it.

## Proposed solution

Add a `tainted: Cell<bool>` flag to `ScopedResolver`. The flag tracks
whether the current resolution chain has touched any local-only type.

### Resolution flow

1. The public `get()` entry point resets `tainted` to `false`.
2. When `resolve()` finds a type in the local `TypeMap` (inserted or
   previously resolved as tainted), it sets `tainted = true`.
3. Finding a type in ancestors does NOT set the flag.
4. After resolving all inputs for a new type, check the flag:
   - **Tainted** → store in the local `TypeMap` (depends on scoped data).
   - **Not tainted** → store in `ancestors[0]` via `get_or_insert()`
     (only depends on shared data; siblings can reuse it).

### Walk-through

| Resolution        | Inputs from                              | Tainted? | Stored in    |
|-------------------|------------------------------------------|----------|--------------|
| `CountedClient`   | `Validator` (ancestor), `Clock` (ancestor) | No       | shared parent |
| `CorrelationVector` | `RequestContext` (local)               | Yes      | local        |
| `RequestHandler`  | `Client` (ancestor), `CorrelationVector` (local) | Yes | local        |

### Trade-offs

- One `Cell<bool>` read/write per dependency lookup — negligible overhead.
- No allocation, no TypeId tracking.
- `get_or_insert` on `SharedTypeMap` already exists and handles the
  concurrent-insert race by keeping the first value.

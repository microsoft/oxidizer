# Observed interest query

`Sink::is_interested` asks processors directly using the supplied event description.
It returns when any processor accepts, including across composite leaves, without entering sampling or delivery.

The query does not share decisions with emission. Emission uses its own sampling context and recipient bitmask, following the [sampling design](../DESIGN.md).
Keeping the query independent lets external sources check interest before collecting fields without reserving recipients or consuming a sampling decision.

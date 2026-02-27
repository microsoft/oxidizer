# TODO

- Should this crate be using ohno instead of std::fs::Error?

- Should this crate be using any types from std::fs in its public API or should we just clone the types so we're isolated?

- Would be nice to enhance bytesbuf so the file crate doesn't need unsafe blocks to get max perf.

- Could easily make the number of worker threads configurable. Should we?

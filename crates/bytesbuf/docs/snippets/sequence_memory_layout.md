# Memory layout

A byte sequence represented by `bytesbuf` types may consist of any number
of separate byte slices, each of which contains one or more bytes of data.

There is no upper or lower bound on the length of each slice of bytes. At one extreme,
a byte sequence may be entirely represented by a single slice of bytes. At the opposite
extreme, it is legal for each byte to be represented by a separate non-consecutive slice.

Examples of legal memory layouts for the byte sequence `b'Hello'`:

* `['H', 'e', 'l', 'l', 'o']`
* `['H', 'e'], ['l', 'l', 'o']`
* `['H'], ['e'], ['l'], ['l'], ['o']`

Code using these APIs is required to work with any memory layout, as there are
no guarantees on which layout will be used for which byte sequences.

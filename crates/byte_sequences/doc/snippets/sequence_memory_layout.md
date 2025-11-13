# Memory layout

A byte sequence backed by I/O memory may consist of any number of spans of consecutive bytes.

There is no upper or lower bound on the length of each span of bytes. At one extreme, the I/O
subsystem may allocate a single span of memory to hold all the data. At the opposite extreme, it is
legal for the I/O subsystem to create byte where byte is stored as a separate allocation.
Higher level APIs are required not assume any specific block size

Examples of how `b'Hello'` may be stored in I/O memory:

* `['H', 'e', 'l', 'l', 'o']`
* `['H', 'e'], ['l', 'l', 'o']`
* `['H'], ['e'], ['l'], ['l'], ['o']`

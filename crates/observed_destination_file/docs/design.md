# File destination design

Emission and output have separate bounds. The record queue limits queued SDK
records; emission clones and tries to enqueue without waiting for file I/O.
A full or disconnected queue drops the newest record and counts the loss.
The worker's reusable byte buffer instead coalesces formatted output. Zero
capacity bypasses byte buffering, not the record queue. Rotation observes bytes
only when they reach the native writer; batches and oversized writes can exceed
the rotation threshold.

One worker owns formatting, the resource header, byte buffering, and the writer.
It checks controls between bounded record-draining batches so sustained emission
does not starve flush or shutdown. Explicit flush drains queued records and
flushes the writer before acknowledging. Automatic deadlines are based on
scheduled or explicit flush attempts, not incoming records or capacity-triggered
writes. Idle waits use the earlier pending-byte or dropped-record-report deadline;
empty buffers need no automatic I/O. A flush provides OS visibility, not durability.

Errors on the asynchronous path are remembered for the next explicit flush or
shutdown, then reported once. A failed write can have committed an unknown prefix:
discarding the rest of that batch avoids replay and duplication but may lose
unwritten records. Later records can still be written. A downstream flush failure
does not replay bytes already written.

Shutdown stops acceptance, requests a final drain and flush, and transfers the
acknowledgement to worker teardown. The worker drops its writer before sending
that acknowledgement; only then does the caller join. A caller timeout does not
cancel blocked I/O or join the thread. Later shutdown and flush calls are no-ops.

Keep scheduling and error-policy tests deterministic with Tick's controlled clock
and memory writers. The fake opener records registration options and shares one
output stream regardless of path; it does not model rotation, compression, or
filesystem behavior. Idle-flush coverage connects the processor to the worker's
channels and drives the loop with a controlled clock and scripted blocking
receives, checking the emitted wake signal and memory-writer output before
teardown without depending on thread scheduling or wall-clock polling.
Public-API integration tests retain real-thread native append/truncate,
output-visibility, retention, and gzip coverage. When changing the worker, test
both queue/control responsiveness and byte-buffer deadlines; when changing opening
options, verify the native boundary as well as fake call recording. Run both
feature-off and rolling-feature configurations so neither masks the other's build.

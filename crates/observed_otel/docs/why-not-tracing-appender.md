# Why not `tracing-appender`?

We evaluated replacing the file destination's crossbeam channel + worker thread
with `tracing-appender`'s `NonBlocking` / `RollingFileAppender`. It is not a
good fit for the following reasons.

## Missing rolling-file features

| Feature | Current (`file-rotate`) | `tracing-appender` |
|---|---|---|
| Size-based rotation | Yes (default 10 MB) | No (time-based only) |
| Age-based retention | Yes (`std::time::Duration`) | No (file-count only) |
| Gzip compression on rotate | Yes | No |

Size-based rotation is the primary rotation strategy; time-based rotation alone
does not meet our requirements.

## No control channel

The `BackgroundLogProcessor` sends `Flush`-with-ack and `SetResource` messages
to the worker. `tracing-appender` has no equivalent — `WorkerGuard` only flushes
on drop, and there is no way to update resource headers mid-stream.

## Conclusion

The current crossbeam channel + `file-rotate` approach already provides
everything `NonBlocking` does, plus structured-data deferral, control messages,
and richer rolling-file capabilities. Adopting `tracing-appender` would lose
features without meaningful gains.

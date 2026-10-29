# Changelog

## [0.4.0] - 2026-10-29

- 🔧 Maintenance

  - Now requires `0.1.3` of `thread_aware_core`

- ✨ Features

  - integrate I/O drivers ([#829](https://github.com/microsoft/oxidizer/pull/829))

- 📚 Documentation

  - clarify I/O context initialization contract ([#912](https://github.com/microsoft/oxidizer/pull/912))

## [0.3.0] - 2026-10-20

- Replace the I/O provider contract with direct `IoContext::driver` creation, `DriverInstance`, owner wakers, and consuming shutdown.
- Update local dependency requirements: thread_aware_core ^0.1.1 -> 0.1.2.

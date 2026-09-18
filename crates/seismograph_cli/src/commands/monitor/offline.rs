// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use super::Error;
use super::data::CapturedSnapshot;

pub(super) struct Loader {
    path: PathBuf,
    receiver: Receiver<Result<Box<CapturedSnapshot>, Error>>,
}

impl Loader {
    pub(super) fn start(path: PathBuf, file: File) -> Result<Self, Error> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_path = path.clone();
        thread::Builder::new()
            .name("seismograph-file".into())
            .spawn(move || {
                let result = load(&worker_path, file);
                // Quitting the viewer deliberately disconnects the receiver.
                let _receiver_closed = sender.send(result);
            })
            .map_err(|error| Error::snapshot_file(&path, error))?;
        Ok(Self { path, receiver })
    }

    pub(super) fn poll(&self) -> Option<Result<Box<CapturedSnapshot>, Error>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(Error::snapshot_file(&self.path, "snapshot loader stopped unexpectedly"))),
        }
    }
}

fn load(path: &Path, file: File) -> Result<Box<CapturedSnapshot>, Error> {
    load_with_progress(path, file, &mut |_| {})
}

fn load_with_progress(
    path: &Path,
    mut file: File,
    progress: &mut impl FnMut(super::snapshot::Phase),
) -> Result<Box<CapturedSnapshot>, Error> {
    use super::snapshot::Phase;
    progress(Phase::Read);
    let length = file.metadata().map_err(|error| Error::snapshot_file(path, error))?.len();
    let length = usize::try_from(length).map_err(|error| Error::snapshot_file(path, error))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|error| Error::snapshot_file(path, error))?;
    file.read_to_end(&mut bytes).map_err(|error| Error::snapshot_file(path, error))?;
    progress(Phase::DecodeContainer);
    let decoded = seismograph::snapshot::decode(&bytes).map_err(|error| Error::snapshot_file(path, error))?;
    // The native decoder owns its data. Release the large input before building summaries.
    progress(Phase::ReleaseInput);
    drop(bytes);
    super::snapshot::prepare_with_progress(decoded, progress).map_err(|error| Error::snapshot_file(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("offline-{}-{name}.seismograph", std::process::id()))
    }

    fn native_bytes(id: seismograph::snapshot::SourceId, data: &[u8]) -> Vec<u8> {
        // Version 1 native container: no threads/events, one source.
        let mut bytes = b"SEISMOG\0".to_vec();
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&id.get().to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&u64::try_from(data.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(b"test");
        bytes.extend_from_slice(data);
        bytes
    }

    fn write(name: &str, bytes: &[u8]) -> PathBuf {
        let path = path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access")]
    fn loads_native_allocator_file_without_inventing_a_capture_time() {
        let allocator = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(0, 1, 0));
        let mut data = vec![0; seismograph_rallocator::encoded_len(&allocator).unwrap()];
        seismograph_rallocator::encode(&allocator, &mut data).unwrap();
        let bytes = native_bytes(seismograph_rallocator::source::ID, &data);
        let path = write("native file with spaces", &bytes);
        let loader = Loader::start(path.clone(), File::open(&path).unwrap()).unwrap();
        let snapshot = loader.receiver.recv_timeout(std::time::Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(
            (
                snapshot.memory.is_some(),
                snapshot.allocations.is_some(),
                snapshot.heap_error,
                snapshot.captured_at,
                snapshot.captured_instant,
            ),
            (true, true, None, None, None),
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access")]
    fn bad_container_and_sources_report_the_input_path() {
        for (name, bytes, expected) in [
            ("bad-container", b"not a snapshot".to_vec(), "malformed"),
            (
                "bad-allocator",
                native_bytes(seismograph_rallocator::source::ID, b"invalid"),
                "invalid rallocator snapshot",
            ),
            (
                "bad-runtime",
                native_bytes(seismograph_runtime::snapshot::source::ID, b"invalid"),
                "invalid runtime snapshot",
            ),
        ] {
            let path = write(name, &bytes);
            let error = load(&path, File::open(&path).unwrap()).err().unwrap().to_string();
            assert!(error.contains(&path.display().to_string()), "{error}");
            assert!(error.contains(expected), "{error}");
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access")]
    fn missing_file_fails_before_terminal_initialization() {
        let path = path("missing");
        let error = super::super::view(super::super::ViewArgs {
            snapshot_file: path.clone(),
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains(&path.display().to_string()));
    }

    #[test]
    fn loader_reports_waiting_and_worker_failure() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let loader = Loader {
            path: path("worker"),
            receiver,
        };
        assert!(loader.poll().is_none());
        drop(sender);
        assert!(loader.poll().unwrap().err().unwrap().to_string().contains("stopped unexpectedly"));
    }

    #[test]
    fn loader_returns_the_worker_result_before_reporting_disconnection() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let path = path("completed-worker");
        let loader = Loader {
            path: path.clone(),
            receiver,
        };
        sender.send(Err(Error::snapshot_file(&path, "invalid source"))).unwrap();
        drop(sender);

        assert_eq!(
            loader.poll().unwrap().err().unwrap().to_string(),
            Error::snapshot_file(&path, "invalid source").to_string(),
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires filesystem access")]
    fn file_loading_reports_ordered_progress_through_ready() {
        let bytes = native_bytes(seismograph::snapshot::SourceId::new(1), b"unknown source");
        let path = write("progress", &bytes);
        let mut phases = Vec::new();
        let snapshot = load_with_progress(&path, File::open(&path).unwrap(), &mut |phase| {
            phases.push(format!("{phase:?}"));
        })
        .unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(
            (
                phases,
                snapshot.memory.is_none(),
                snapshot.allocations.is_none(),
                snapshot.heap_error.is_some(),
                snapshot.captured_at,
                snapshot.captured_instant,
            ),
            (
                [
                    "Read",
                    "DecodeContainer",
                    "ReleaseInput",
                    "AllocationIndex",
                    "Heaps",
                    "Allocations",
                    "Symbols",
                    "Primitives",
                    "Runtime",
                    "Io",
                    "Cache",
                    "Threads",
                    "ReleaseEvents",
                    "Ready",
                ]
                .map(String::from)
                .to_vec(),
                true,
                true,
                true,
                None,
                None,
            ),
        );
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Manual profiling, not an automated test.
    #[ignore = "requires SEISMOGRAPH_TEST_SNAPSHOT to name a large local capture"]
    fn large_file_load() {
        let path = PathBuf::from(std::env::var_os("SEISMOGRAPH_TEST_SNAPSHOT").unwrap());
        if std::env::var_os("SEISMOGRAPH_PROFILE_ALLOCATIONS").is_some() {
            super::super::profile::enable_allocations();
        }
        let started = std::time::Instant::now();
        let mut previous = None;
        let mut phase_started = started;
        let mut phase_sample = super::super::profile::Sample::now();
        let snapshot = load_with_progress(&path, File::open(&path).unwrap(), &mut |phase| {
            let sample = super::super::profile::Sample::now();
            if let Some(previous) = previous {
                println!(
                    "Phase {previous:?}: {:.3}s; {}",
                    phase_started.elapsed().as_secs_f64(),
                    sample.describe_since(&phase_sample)
                );
            }
            previous = Some(phase);
            phase_started = std::time::Instant::now();
            phase_sample = sample;
        })
        .unwrap();
        println!(
            "Loaded {} in {:.1}s: {} threads, {} total events, {} retained events, {} allocation hotspots",
            path.display(),
            started.elapsed().as_secs_f64(),
            snapshot.threads.threads.len(),
            snapshot.primitives.total_events,
            snapshot.threads.threads.iter().map(|thread| thread.retained_events).sum::<u64>(),
            snapshot.allocations.as_ref().map_or(0, |allocations| allocations.hotspots.len()),
        );
        assert_eq!((snapshot.captured_at, snapshot.captured_instant), (None, None));
        let mut app = super::super::app::App::offline(path);
        app.finish_offline_load(snapshot);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(180, 60)).unwrap();
        for tab in '1'..='8' {
            app.handle_key(crossterm::event::KeyCode::Char(tab));
            let render_started = std::time::Instant::now();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            println!("Rendered snapshot tab {tab} in {:.3}s", render_started.elapsed().as_secs_f64());
        }
        assert!(app.handle_key(crossterm::event::KeyCode::Esc));
        let drop_started = std::time::Instant::now();
        drop(app);
        println!(
            "Drop view model: {:.3}s; end-to-end: {:.3}s",
            drop_started.elapsed().as_secs_f64(),
            started.elapsed().as_secs_f64()
        );
    }
}

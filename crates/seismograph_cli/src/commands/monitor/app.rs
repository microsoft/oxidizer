// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::KeyCode;
use performables::sync::channel::{Receiver, Sender, unbounded};
use seismograph_protocol::message::{EventBufferDisposition, RecorderStatistics, RecordingConfiguration, SnapshotOptions};
use seismograph_protocol::monitor::MonitorDescriptor;

use super::client::{capture_snapshot, discover, recorder_statistics, save_snapshot, set_recording};
use super::data::{
    AllocationSnapshot, AllocationSort, AllocationStackFilter, CapturedSnapshot, MemorySnapshot, MemoryTier, PrimitiveSort,
    RuntimeSnapshot, RuntimeTaskSort,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const MAX_ACTIVITY_SAMPLES: usize = 120;
pub(super) const EVENT_BUFFER_CAPACITIES: [u32; 15] = [
    64, 128, 256, 512, 1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576,
];
pub(super) const EVENT_SAMPLING_RATES: [u32; 19] = [
    1, 2, 4, 8, 16, 20, 32, 64, 100, 128, 256, 512, 1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536,
];

pub(super) struct Instance {
    pub(super) descriptor: MonitorDescriptor,
    pub(super) recording: RecordingConfiguration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActivitySample {
    pub(super) captured_at: Instant,
    pub(super) events_per_second: u64,
    pub(super) total_events: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RecordingConfigurationPopup {
    pub(super) draft: RecordingConfiguration,
    pub(super) selected: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecordingConfigurationField {
    AllocationRecording,
    AllocationBacktraces,
    AllocationSampling,
    GeneralRecording,
    GeneralBacktraces,
    GeneralSampling,
    ArcDereferenceRecording,
    ArcDereferenceBacktraces,
    ArcDereferenceSampling,
    RuntimeTaskRecording,
    RuntimeTaskBacktraces,
    IoRecording,
    IoBacktraces,
    IoSampling,
    CacheRecording,
    CacheBacktraces,
    CacheSampling,
    EventBufferCapacity,
    Apply,
    Cancel,
}

impl RecordingConfigurationField {
    pub(super) const ALL: [Self; 20] = [
        Self::AllocationRecording,
        Self::AllocationBacktraces,
        Self::AllocationSampling,
        Self::GeneralRecording,
        Self::GeneralBacktraces,
        Self::GeneralSampling,
        Self::ArcDereferenceRecording,
        Self::ArcDereferenceBacktraces,
        Self::ArcDereferenceSampling,
        Self::RuntimeTaskRecording,
        Self::RuntimeTaskBacktraces,
        Self::IoRecording,
        Self::IoBacktraces,
        Self::IoSampling,
        Self::CacheRecording,
        Self::CacheBacktraces,
        Self::CacheSampling,
        Self::EventBufferCapacity,
        Self::Apply,
        Self::Cancel,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::AllocationRecording => "Allocations recording",
            Self::AllocationBacktraces => "Allocations backtraces",
            Self::AllocationSampling => "Allocations sampling",
            Self::GeneralRecording => "General events recording",
            Self::GeneralBacktraces => "General events backtraces",
            Self::GeneralSampling => "General events sampling",
            Self::ArcDereferenceRecording => "Arc dereference recording",
            Self::ArcDereferenceBacktraces => "Arc dereference backtraces",
            Self::ArcDereferenceSampling => "Arc dereference sampling",
            Self::RuntimeTaskRecording => "Runtime task recording",
            Self::RuntimeTaskBacktraces => "Runtime task backtraces",
            Self::IoRecording => "I/O recording",
            Self::IoBacktraces => "I/O backtraces",
            Self::IoSampling => "I/O resource sampling",
            Self::CacheRecording => "Cache recording",
            Self::CacheBacktraces => "Cache backtraces",
            Self::CacheSampling => "Cache tier sampling",
            Self::EventBufferCapacity => "Event buffer capacity",
            Self::Apply => "OK",
            Self::Cancel => "Cancel",
        }
    }

    pub(super) fn value(self, configuration: RecordingConfiguration) -> String {
        match self {
            Self::AllocationRecording => toggle_label(configuration.allocations.enabled),
            Self::AllocationBacktraces => toggle_label(configuration.allocations.capture_backtraces),
            Self::AllocationSampling => sampling_label(configuration.allocations.sampling_one_in),
            Self::GeneralRecording => toggle_label(configuration.general_events.enabled),
            Self::GeneralBacktraces => toggle_label(configuration.general_events.capture_backtraces),
            Self::GeneralSampling => sampling_label(configuration.general_events.sampling_one_in),
            Self::ArcDereferenceRecording => toggle_label(configuration.arc_dereferences.enabled),
            Self::ArcDereferenceBacktraces => toggle_label(configuration.arc_dereferences.capture_backtraces),
            Self::ArcDereferenceSampling => sampling_label(configuration.arc_dereferences.sampling_one_in),
            Self::RuntimeTaskRecording => toggle_label(configuration.runtime_tasks.enabled),
            Self::RuntimeTaskBacktraces => toggle_label(configuration.runtime_tasks.capture_backtraces),
            Self::IoRecording => toggle_label(configuration.io.enabled),
            Self::IoBacktraces => toggle_label(configuration.io.capture_backtraces),
            Self::IoSampling => sampling_label(configuration.io.sampling_one_in),
            Self::CacheRecording => toggle_label(configuration.cache.enabled),
            Self::CacheBacktraces => toggle_label(configuration.cache.capture_backtraces),
            Self::CacheSampling => sampling_label(configuration.cache.sampling_one_in),
            Self::EventBufferCapacity => format!("{} events / thread", configuration.event_capacity_per_thread),
            Self::Apply | Self::Cancel => String::new(),
        }
    }

    fn adjust(self, configuration: &mut RecordingConfiguration, direction: isize) {
        match self {
            Self::AllocationRecording => configuration.allocations.enabled = !configuration.allocations.enabled,
            Self::AllocationBacktraces => {
                configuration.allocations.capture_backtraces = !configuration.allocations.capture_backtraces;
            }
            Self::AllocationSampling => {
                configuration.allocations.sampling_one_in =
                    adjusted_value(configuration.allocations.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::GeneralRecording => configuration.general_events.enabled = !configuration.general_events.enabled,
            Self::GeneralBacktraces => {
                configuration.general_events.capture_backtraces = !configuration.general_events.capture_backtraces;
            }
            Self::GeneralSampling => {
                configuration.general_events.sampling_one_in =
                    adjusted_value(configuration.general_events.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::ArcDereferenceRecording => {
                configuration.arc_dereferences.enabled = !configuration.arc_dereferences.enabled;
            }
            Self::ArcDereferenceBacktraces => {
                configuration.arc_dereferences.capture_backtraces = !configuration.arc_dereferences.capture_backtraces;
            }
            Self::ArcDereferenceSampling => {
                configuration.arc_dereferences.sampling_one_in =
                    adjusted_value(configuration.arc_dereferences.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::RuntimeTaskRecording => configuration.runtime_tasks.enabled = !configuration.runtime_tasks.enabled,
            Self::RuntimeTaskBacktraces => {
                configuration.runtime_tasks.capture_backtraces = !configuration.runtime_tasks.capture_backtraces;
            }
            Self::IoRecording => configuration.io.enabled = !configuration.io.enabled,
            Self::IoBacktraces => {
                configuration.io.capture_backtraces = !configuration.io.capture_backtraces;
            }
            Self::IoSampling => {
                configuration.io.sampling_one_in = adjusted_value(configuration.io.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::CacheRecording => configuration.cache.enabled = !configuration.cache.enabled,
            Self::CacheBacktraces => {
                configuration.cache.capture_backtraces = !configuration.cache.capture_backtraces;
            }
            Self::CacheSampling => {
                configuration.cache.sampling_one_in = adjusted_value(configuration.cache.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::EventBufferCapacity => {
                configuration.event_capacity_per_thread =
                    adjusted_value(configuration.event_capacity_per_thread, &EVENT_BUFFER_CAPACITIES, direction);
            }
            Self::Apply | Self::Cancel => {}
        }
    }
}

impl RecordingConfigurationPopup {
    pub(super) fn field(self) -> RecordingConfigurationField {
        RecordingConfigurationField::ALL[self.selected]
    }
}

fn toggle_label(enabled: bool) -> String {
    if enabled { "on" } else { "off" }.to_owned()
}

fn sampling_label(sampling_one_in: u32) -> String {
    format!("1/{sampling_one_in} ({})", format_sampling_percentage(sampling_one_in))
}

fn adjusted_value(current: u32, values: &[u32], direction: isize) -> u32 {
    let index = values
        .iter()
        .position(|candidate| *candidate >= current)
        .unwrap_or(values.len() - 1);
    let adjusted = index.saturating_add_signed(direction).min(values.len() - 1);
    values[adjusted]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AllocationViewState {
    pub(super) sort: AllocationSort,
    pub(super) descending: bool,
    pub(super) selected: usize,
    pub(super) stack_scroll: usize,
    pub(super) stack_filter: AllocationStackFilter,
}

impl AllocationViewState {
    const fn new() -> Self {
        Self {
            sort: AllocationSort::Allocations,
            descending: true,
            selected: 0,
            stack_scroll: 0,
            stack_filter: AllocationStackFilter::Application,
        }
    }

    fn reset_position(&mut self) {
        self.selected = 0;
        self.stack_scroll = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveViewState {
    pub(super) focus: PrimitiveFocus,
    pub(super) primitive_selected: usize,
    pub(super) operation_selected: usize,
    pub(super) hotspot_selected: usize,
    pub(super) stack_scroll: usize,
    pub(super) sort: PrimitiveSort,
    pub(super) descending: bool,
    pub(super) stack_filter: AllocationStackFilter,
}

impl PrimitiveViewState {
    const fn new() -> Self {
        Self {
            focus: PrimitiveFocus::Types,
            primitive_selected: 0,
            operation_selected: 0,
            hotspot_selected: 0,
            stack_scroll: 0,
            sort: PrimitiveSort::Events,
            descending: true,
            stack_filter: AllocationStackFilter::Application,
        }
    }

    fn reset_operations(&mut self) {
        self.operation_selected = 0;
        self.reset_hotspots();
    }

    fn reset_hotspots(&mut self) {
        self.hotspot_selected = 0;
        self.stack_scroll = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveFocus {
    Types,
    Operations,
    Hotspots,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HeapViewState {
    pub(super) tier: MemoryTier,
    pub(super) focus: HeapFocus,
    pub(super) bucket_selected: usize,
    pub(super) hotspot_selected: usize,
    pub(super) stack_scroll: usize,
    pub(super) stack_filter: AllocationStackFilter,
}

impl HeapViewState {
    const fn new() -> Self {
        Self {
            tier: MemoryTier::Small,
            focus: HeapFocus::Buckets,
            bucket_selected: 0,
            hotspot_selected: 0,
            stack_scroll: 0,
            stack_filter: AllocationStackFilter::Application,
        }
    }

    fn reset(&mut self) {
        self.focus = HeapFocus::Buckets;
        self.bucket_selected = 0;
        self.reset_hotspot();
    }

    fn reset_hotspot(&mut self) {
        self.hotspot_selected = 0;
        self.stack_scroll = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HeapFocus {
    Buckets,
    Hotspots,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ThreadViewState {
    pub(super) focus: ThreadFocus,
    pub(super) thread_selected: usize,
    pub(super) operation_selected: usize,
    pub(super) participant_selected: usize,
    pub(super) object_selected: usize,
    pub(super) stack_scroll: usize,
    pub(super) stack_filter: AllocationStackFilter,
}

impl ThreadViewState {
    const fn new() -> Self {
        Self {
            focus: ThreadFocus::Threads,
            thread_selected: 0,
            operation_selected: 0,
            participant_selected: 0,
            object_selected: 0,
            stack_scroll: 0,
            stack_filter: AllocationStackFilter::Application,
        }
    }

    fn reset(&mut self) {
        self.focus = ThreadFocus::Threads;
        self.thread_selected = 0;
        self.reset_operation();
    }

    fn reset_operation(&mut self) {
        self.operation_selected = 0;
        self.reset_participant();
    }

    fn reset_participant(&mut self) {
        self.participant_selected = 0;
        self.reset_object();
    }

    fn reset_object(&mut self) {
        self.object_selected = 0;
        self.stack_scroll = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ThreadFocus {
    Threads,
    Operations,
    Participants,
    Objects,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RuntimeViewState {
    pub(super) focus: RuntimeFocus,
    pub(super) worker_selected: usize,
    pub(super) task_selected: usize,
    pub(super) detail_view: RuntimeDetailView,
    pub(super) detail_scroll: usize,
    pub(super) task_sort: RuntimeTaskSort,
    pub(super) task_sort_descending: bool,
}

impl RuntimeViewState {
    const fn new() -> Self {
        Self {
            focus: RuntimeFocus::Workers,
            worker_selected: 0,
            task_selected: 0,
            detail_view: RuntimeDetailView::Details,
            detail_scroll: 0,
            task_sort: RuntimeTaskSort::Polls,
            task_sort_descending: true,
        }
    }

    fn reset(&mut self) {
        self.focus = RuntimeFocus::Workers;
        self.worker_selected = 0;
        self.reset_task();
    }

    fn reset_task(&mut self) {
        self.task_selected = 0;
        self.detail_view = RuntimeDetailView::Details;
        self.detail_scroll = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeFocus {
    Workers,
    Tasks,
    Details,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeDetailView {
    Details,
    SpawnStack,
}

impl RuntimeDetailView {
    pub(super) const fn toggle(self) -> Self {
        match self {
            Self::Details => Self::SpawnStack,
            Self::SpawnStack => Self::Details,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::SpawnStack => "Spawn Stack",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MonitorTab {
    Info,
    Heaps,
    Allocations,
    Primitives,
    Threads,
    Runtime,
}

impl MonitorTab {
    pub(super) const fn index(self) -> usize {
        match self {
            Self::Info => 0,
            Self::Heaps => 1,
            Self::Allocations => 2,
            Self::Primitives => 3,
            Self::Threads => 4,
            Self::Runtime => 5,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Info => Self::Heaps,
            Self::Heaps => Self::Allocations,
            Self::Allocations => Self::Primitives,
            Self::Primitives => Self::Threads,
            Self::Threads => Self::Runtime,
            Self::Runtime => Self::Info,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Info => Self::Runtime,
            Self::Heaps => Self::Info,
            Self::Allocations => Self::Heaps,
            Self::Primitives => Self::Allocations,
            Self::Threads => Self::Primitives,
            Self::Runtime => Self::Threads,
        }
    }
}

pub(super) enum Screen {
    Browse,
    Connected {
        descriptor: MonitorDescriptor,
        recording: RecordingConfiguration,
        tab: MonitorTab,
        snapshot: Option<Box<CapturedSnapshot>>,
    },
}

pub(super) struct App {
    pub(super) instances: Vec<Instance>,
    pub(super) selected: usize,
    pub(super) screen: Screen,
    pub(super) status: String,
    pub(super) allocation_view: AllocationViewState,
    pub(super) heap_view: HeapViewState,
    pub(super) primitive_view: PrimitiveViewState,
    pub(super) thread_view: ThreadViewState,
    pub(super) runtime_view: RuntimeViewState,
    pub(super) activity_samples: VecDeque<ActivitySample>,
    pub(super) recorder_statistics: Option<RecorderStatistics>,
    pub(super) snapshot_options: SnapshotOptions,
    pub(super) snapshot_error: Option<String>,
    pub(super) recording_configuration_popup: Option<RecordingConfigurationPopup>,
    activity_observed_at: Option<Instant>,
    pub(super) capture_started_at: Option<Instant>,
    pub(super) capture_step: Option<CaptureStep>,
    capture_receiver: Option<Receiver<CaptureMessage>>,
    discovery_receiver: Option<Receiver<Result<Vec<Instance>, String>>>,
    statistics_receiver: Option<Receiver<Result<RecorderStatistics, String>>>,
    next_refresh: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CaptureStep {
    Capture,
    Decode,
    Save,
}

impl CaptureStep {
    pub(super) const ALL: [Self; 3] = [Self::Capture, Self::Decode, Self::Save];

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Capture => 0,
            Self::Decode => 1,
            Self::Save => 2,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Capture => "Capture process snapshot",
            Self::Decode => "Decode telemetry",
            Self::Save => "Save snapshot file",
        }
    }

    pub(super) fn progress(self) -> f64 {
        match self {
            Self::Capture => 1.0 / 6.0,
            Self::Decode => 1.0 / 2.0,
            Self::Save => 5.0 / 6.0,
        }
    }
}

enum CaptureMessage {
    Progress(CaptureStep),
    Complete(Result<CaptureOutcome, String>),
}

struct CaptureOutcome {
    snapshot: Box<CapturedSnapshot>,
    status: String,
}

impl App {
    pub(super) fn new() -> Self {
        Self {
            instances: Vec::new(),
            selected: 0,
            screen: Screen::Browse,
            status: String::new(),
            allocation_view: AllocationViewState::new(),
            heap_view: HeapViewState::new(),
            primitive_view: PrimitiveViewState::new(),
            thread_view: ThreadViewState::new(),
            runtime_view: RuntimeViewState::new(),
            activity_samples: VecDeque::new(),
            recorder_statistics: None,
            snapshot_options: SnapshotOptions::default(),
            snapshot_error: None,
            recording_configuration_popup: None,
            activity_observed_at: None,
            capture_started_at: None,
            capture_step: None,
            capture_receiver: None,
            discovery_receiver: None,
            statistics_receiver: None,
            next_refresh: Instant::now(),
        }
    }

    pub(super) const fn next_refresh(&self) -> Instant {
        self.next_refresh
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(super) fn refresh(&mut self) {
        self.next_refresh = Instant::now() + REFRESH_INTERVAL;
        if let Screen::Connected { descriptor, .. } = &self.screen {
            if self.capture_receiver.is_none() && self.statistics_receiver.is_none() {
                self.start_recorder_statistics(descriptor.clone());
            }
            return;
        }
        if self.discovery_receiver.is_none() {
            self.start_discovery();
        }
    }

    pub(super) fn handle_key(&mut self, code: KeyCode) -> bool {
        if matches!(code, KeyCode::Char('q' | 'Q')) {
            return true;
        }
        let capture_in_progress = self.capture_receiver.is_some();
        if self.recording_configuration_popup.is_some() {
            self.handle_recording_configuration_key(code);
            return false;
        }
        if code == KeyCode::Char('s') {
            if capture_in_progress {
                return false;
            }
            let capture = match &self.screen {
                Screen::Connected { descriptor, .. } => Some(descriptor.clone()),
                Screen::Browse => None,
            };
            if let Some(descriptor) = capture {
                self.start_snapshot_capture(descriptor, self.snapshot_options);
            }
            return false;
        }
        match &mut self.screen {
            Screen::Browse => match code {
                KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                KeyCode::Down => {
                    self.selected = (self.selected + 1).min(self.instances.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    if let Some(instance) = self.instances.get(self.selected) {
                        self.activity_samples.clear();
                        self.recorder_statistics = None;
                        self.activity_observed_at = None;
                        self.screen = Screen::Connected {
                            descriptor: instance.descriptor.clone(),
                            recording: instance.recording,
                            tab: MonitorTab::Info,
                            snapshot: None,
                        };
                        self.snapshot_error = None;
                        self.status.clear();
                    }
                }
                KeyCode::Char('r') => self.refresh(),
                _ => {}
            },
            Screen::Connected {
                recording, tab, snapshot, ..
            } => match code {
                _ if *tab == MonitorTab::Heaps && handle_heap_key(code, &mut self.heap_view, snapshot.as_deref()) => {}
                _ if *tab == MonitorTab::Allocations && handle_allocation_key(code, &mut self.allocation_view, snapshot.as_deref()) => {}
                _ if *tab == MonitorTab::Primitives && handle_primitive_key(code, &mut self.primitive_view, snapshot.as_deref()) => {}
                _ if *tab == MonitorTab::Threads && handle_thread_key(code, &mut self.thread_view, snapshot.as_deref()) => {}
                _ if *tab == MonitorTab::Runtime && handle_runtime_key(code, &mut self.runtime_view, snapshot.as_deref()) => {}
                KeyCode::Esc => {
                    self.screen = Screen::Browse;
                    self.refresh();
                }
                KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => *tab = tab.previous(),
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => *tab = tab.next(),
                KeyCode::Char('1') => *tab = MonitorTab::Info,
                KeyCode::Char('2') => *tab = MonitorTab::Heaps,
                KeyCode::Char('3') => *tab = MonitorTab::Allocations,
                KeyCode::Char('4') => *tab = MonitorTab::Primitives,
                KeyCode::Char('5') => *tab = MonitorTab::Threads,
                KeyCode::Char('6') => *tab = MonitorTab::Runtime,
                KeyCode::Char('d') => {
                    self.snapshot_options.event_buffers = next_buffer_disposition(self.snapshot_options.event_buffers);
                    self.status = format!("Snapshot buffers: {:?}", self.snapshot_options.event_buffers);
                }
                KeyCode::Char('c') if !capture_in_progress => {
                    self.recording_configuration_popup = Some(RecordingConfigurationPopup {
                        draft: *recording,
                        selected: 0,
                    });
                }
                _ => {}
            },
        }
        false
    }

    fn handle_recording_configuration_key(&mut self, code: KeyCode) {
        let Some(popup) = &mut self.recording_configuration_popup else {
            return;
        };
        match code {
            KeyCode::Up => popup.selected = popup.selected.saturating_sub(1),
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(RecordingConfigurationField::ALL.len() - 1);
            }
            KeyCode::Left => popup.field().adjust(&mut popup.draft, -1),
            KeyCode::Right | KeyCode::Char(' ') => popup.field().adjust(&mut popup.draft, 1),
            KeyCode::Esc => self.recording_configuration_popup = None,
            KeyCode::Enter => match popup.field() {
                RecordingConfigurationField::Apply => {
                    let configuration = popup.draft;
                    self.apply_recording_configuration(configuration);
                }
                RecordingConfigurationField::Cancel => {
                    self.recording_configuration_popup = None;
                    self.status = "Recording configuration unchanged".into();
                }
                field => field.adjust(&mut popup.draft, 1),
            },
            _ => {}
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn apply_recording_configuration(&mut self, configuration: RecordingConfiguration) {
        let Screen::Connected { descriptor, recording, .. } = &mut self.screen else {
            self.recording_configuration_popup = None;
            return;
        };
        match set_recording(descriptor, configuration) {
            Ok(()) => {
                *recording = configuration;
                self.recording_configuration_popup = None;
                self.status = "Recording configuration applied".into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    pub(super) fn poll_snapshot_capture(&mut self) {
        loop {
            let message = match receive_capture_message(self.capture_receiver.as_ref()) {
                Ok(Some(message)) => message,
                Ok(None) => return,
                Err(error) => {
                    self.finish_snapshot_capture(Err(error));
                    return;
                }
            };
            match message {
                CaptureMessage::Progress(step) => self.capture_step = Some(step),
                CaptureMessage::Complete(result) => {
                    self.finish_snapshot_capture(result);
                    return;
                }
            }
        }
    }

    pub(super) fn poll_discovery(&mut self) {
        let Some(result) = receive_worker_result(self.discovery_receiver.as_ref(), "Monitor discovery") else {
            return;
        };
        self.discovery_receiver = None;
        if !matches!(self.screen, Screen::Browse) {
            return;
        }
        match result {
            Ok(instances) => {
                let selected_id = self.instances.get(self.selected).map(|instance| instance.descriptor.instance_id);
                self.instances = instances;
                self.selected = selected_id
                    .and_then(|id| self.instances.iter().position(|instance| instance.descriptor.instance_id == id))
                    .unwrap_or(0)
                    .min(self.instances.len().saturating_sub(1));
                self.status = if self.instances.is_empty() {
                    "No reachable Seismograph monitors found".into()
                } else {
                    format!("{} application(s) available", self.instances.len())
                };
            }
            Err(error) => self.status = error,
        }
    }

    pub(super) fn poll_recorder_statistics(&mut self) {
        let Some(result) = receive_worker_result(self.statistics_receiver.as_ref(), "Recorder statistics") else {
            return;
        };
        self.statistics_receiver = None;
        match result {
            Ok(statistics) => self.record_activity(statistics),
            Err(error) => self.status = error,
        }
    }

    fn finish_snapshot_capture(&mut self, result: Result<CaptureOutcome, String>) {
        self.capture_receiver = None;
        self.capture_started_at = None;
        self.capture_step = None;
        match result {
            Ok(outcome) => {
                self.snapshot_error = None;
                if let Screen::Connected { snapshot, .. } = &mut self.screen {
                    *snapshot = Some(outcome.snapshot);
                    self.heap_view.reset();
                    self.allocation_view.reset_position();
                    self.thread_view.reset();
                    self.runtime_view.reset();
                }
                self.status = outcome.status;
            }
            Err(error) => {
                self.snapshot_error = Some(error.clone());
                self.status = error;
            }
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_snapshot_capture(&mut self, descriptor: MonitorDescriptor, options: SnapshotOptions) {
        let (sender, receiver) = unbounded();
        match thread::Builder::new().name("seismograph-snapshot".into()).spawn(move || {
            let result = capture_connected_snapshot(&descriptor, options, &sender);
            let _receiver_closed = sender.send_sync(CaptureMessage::Complete(result));
        }) {
            Ok(_worker) => {
                self.snapshot_error = None;
                self.capture_started_at = Some(Instant::now());
                self.capture_step = Some(CaptureStep::Capture);
                self.capture_receiver = Some(receiver);
                self.status.clear();
            }
            Err(error) => self.status = format!("failed to start snapshot capture: {error}"),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_discovery(&mut self) {
        let (sender, receiver) = unbounded();
        match thread::Builder::new().name("seismograph-discovery".into()).spawn(move || {
            let result = discover().map_err(|error| error.to_string());
            let _receiver_closed = sender.send_sync(result);
        }) {
            Ok(_worker) => self.discovery_receiver = Some(receiver),
            Err(error) => self.status = format!("failed to start monitor discovery worker: {error}"),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_recorder_statistics(&mut self, descriptor: MonitorDescriptor) {
        let (sender, receiver) = unbounded();
        match thread::Builder::new().name("seismograph-statistics".into()).spawn(move || {
            let result = recorder_statistics(&descriptor).map_err(|error| error.to_string());
            let _receiver_closed = sender.send_sync(result);
        }) {
            Ok(_worker) => self.statistics_receiver = Some(receiver),
            Err(error) => self.status = format!("failed to start recorder statistics worker: {error}"),
        }
    }

    fn record_activity(&mut self, statistics: RecorderStatistics) {
        let captured_at = Instant::now();
        if let Some((previous, observed_at)) = self.recorder_statistics.as_ref().zip(self.activity_observed_at) {
            let elapsed = captured_at.saturating_duration_since(observed_at);
            self.activity_samples.push_back(ActivitySample {
                captured_at,
                events_per_second: activity_rate(previous.total_events, statistics.total_events, elapsed),
                total_events: statistics.total_events,
            });
        }
        while self.activity_samples.len() > MAX_ACTIVITY_SAMPLES {
            self.activity_samples.pop_front();
        }
        self.activity_observed_at = Some(captured_at);
        self.recorder_statistics = Some(statistics);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn receive_capture_message(receiver: Option<&Receiver<CaptureMessage>>) -> Result<Option<CaptureMessage>, String> {
    match receiver.map(Receiver::try_recv) {
        Some(Ok(message)) => Ok(Some(message)),
        Some(Err(error)) if error.is_empty() => Ok(None),
        None => Ok(None),
        Some(Err(error)) if error.is_closed() => Err("Snapshot capture worker stopped unexpectedly".into()),
        Some(Err(error)) => Err(format!("Snapshot capture channel failed: {error}")),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn receive_worker_result<T>(receiver: Option<&Receiver<Result<T, String>>>, worker: &str) -> Option<Result<T, String>> {
    match receiver.map(Receiver::try_recv) {
        Some(Ok(result)) => Some(result),
        Some(Err(error)) if error.is_empty() => None,
        None => None,
        Some(Err(error)) if error.is_closed() => Some(Err(format!("{worker} worker stopped unexpectedly"))),
        Some(Err(error)) => Some(Err(format!("{worker} channel failed: {error}"))),
    }
}

fn activity_rate(previous_total: u64, current_total: u64, elapsed: Duration) -> u64 {
    let elapsed_millis = elapsed.as_millis();
    if elapsed_millis == 0 {
        return 0;
    }
    let event_delta = current_total.saturating_sub(previous_total);
    let rate = u128::from(event_delta).saturating_mul(1_000) / elapsed_millis;
    u64::try_from(rate).unwrap_or(u64::MAX)
}

fn handle_allocation_key(code: KeyCode, view: &mut AllocationViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    match code {
        KeyCode::Up => {
            view.selected = view.selected.saturating_sub(1);
            view.stack_scroll = 0;
        }
        KeyCode::Down => {
            let hotspot_count = snapshot
                .and_then(|capture| capture.allocations.as_ref())
                .map_or(0, |allocations| allocations.hotspots.len());
            view.selected = (view.selected + 1).min(hotspot_count.saturating_sub(1));
            view.stack_scroll = 0;
        }
        KeyCode::PageUp => view.stack_scroll = view.stack_scroll.saturating_sub(1),
        KeyCode::PageDown => view.stack_scroll = view.stack_scroll.saturating_add(1),
        KeyCode::Char('[') => {
            view.sort = view.sort.previous();
            view.reset_position();
        }
        KeyCode::Char(']') => {
            view.sort = view.sort.next();
            view.reset_position();
        }
        KeyCode::Char('r') => {
            view.descending = !view.descending;
            view.reset_position();
        }
        KeyCode::Char('f') => {
            view.stack_filter = view.stack_filter.toggle();
            view.stack_scroll = 0;
        }
        _ => return false,
    }
    true
}

fn handle_heap_key(code: KeyCode, view: &mut HeapViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let memory = snapshot.and_then(|snapshot| snapshot.memory.as_ref());
    let tier = memory.and_then(|memory| memory.tiers.iter().find(|tier| tier.kind == view.tier));
    let bucket = tier.and_then(|tier| tier.buckets.get(view.bucket_selected));
    match code {
        KeyCode::Char('[') => {
            view.tier = view.tier.previous();
            view.reset();
        }
        KeyCode::Char(']') => {
            view.tier = view.tier.next();
            view.reset();
        }
        KeyCode::Up => match view.focus {
            HeapFocus::Buckets => {
                view.bucket_selected = view.bucket_selected.saturating_sub(1);
                view.reset_hotspot();
            }
            HeapFocus::Hotspots => {
                view.hotspot_selected = view.hotspot_selected.saturating_sub(1);
                view.stack_scroll = 0;
            }
        },
        KeyCode::Down => match view.focus {
            HeapFocus::Buckets => {
                let count = tier.map_or(0, |tier| tier.buckets.len());
                view.bucket_selected = (view.bucket_selected + 1).min(count.saturating_sub(1));
                view.reset_hotspot();
            }
            HeapFocus::Hotspots => {
                let count = bucket.map_or(0, |bucket| bucket.hotspots.len());
                view.hotspot_selected = (view.hotspot_selected + 1).min(count.saturating_sub(1));
                view.stack_scroll = 0;
            }
        },
        KeyCode::Enter => view.focus = HeapFocus::Hotspots,
        KeyCode::Backspace => {
            if view.focus == HeapFocus::Buckets {
                return false;
            }
            view.focus = HeapFocus::Buckets;
        }
        KeyCode::PageUp => view.stack_scroll = view.stack_scroll.saturating_sub(1),
        KeyCode::PageDown => view.stack_scroll = view.stack_scroll.saturating_add(1),
        KeyCode::Char('f') => {
            view.stack_filter = view.stack_filter.toggle();
            view.stack_scroll = 0;
        }
        _ => return false,
    }
    true
}

fn handle_primitive_key(code: KeyCode, view: &mut PrimitiveViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let primitives = snapshot.map(|snapshot| &snapshot.primitives);
    let group = primitives.and_then(|primitives| primitives.groups.get(view.primitive_selected));
    let operations = group.map(|group| group.sorted_operations(view.sort, view.descending));
    let operation = operations
        .as_ref()
        .and_then(|operations| operations.get(view.operation_selected))
        .copied();
    match code {
        KeyCode::Up => match view.focus {
            PrimitiveFocus::Types => {
                view.primitive_selected = view.primitive_selected.saturating_sub(1);
                view.reset_operations();
            }
            PrimitiveFocus::Operations => {
                view.operation_selected = view.operation_selected.saturating_sub(1);
                view.reset_hotspots();
            }
            PrimitiveFocus::Hotspots => {
                view.hotspot_selected = view.hotspot_selected.saturating_sub(1);
                view.stack_scroll = 0;
            }
        },
        KeyCode::Down => match view.focus {
            PrimitiveFocus::Types => {
                let count = primitives.map_or(0, |primitives| primitives.groups.len());
                view.primitive_selected = (view.primitive_selected + 1).min(count.saturating_sub(1));
                view.reset_operations();
            }
            PrimitiveFocus::Operations => {
                let count = operations.as_ref().map_or(0, Vec::len);
                view.operation_selected = (view.operation_selected + 1).min(count.saturating_sub(1));
                view.reset_hotspots();
            }
            PrimitiveFocus::Hotspots => {
                let count = operation.map_or(0, |operation| operation.hotspots.len());
                view.hotspot_selected = (view.hotspot_selected + 1).min(count.saturating_sub(1));
                view.stack_scroll = 0;
            }
        },
        KeyCode::Enter => {
            view.focus = match view.focus {
                PrimitiveFocus::Types => PrimitiveFocus::Operations,
                PrimitiveFocus::Operations | PrimitiveFocus::Hotspots => PrimitiveFocus::Hotspots,
            };
        }
        KeyCode::Backspace => {
            view.focus = match view.focus {
                PrimitiveFocus::Types => return false,
                PrimitiveFocus::Operations => PrimitiveFocus::Types,
                PrimitiveFocus::Hotspots => PrimitiveFocus::Operations,
            };
        }
        KeyCode::PageUp => view.stack_scroll = view.stack_scroll.saturating_sub(1),
        KeyCode::PageDown => view.stack_scroll = view.stack_scroll.saturating_add(1),
        KeyCode::Char('[') => {
            view.sort = view.sort.previous();
            view.reset_operations();
        }
        KeyCode::Char(']') => {
            view.sort = view.sort.next();
            view.reset_operations();
        }
        KeyCode::Char('r') => {
            view.descending = !view.descending;
            view.reset_operations();
        }
        KeyCode::Char('f') => {
            view.stack_filter = view.stack_filter.toggle();
            view.stack_scroll = 0;
        }
        _ => return false,
    }
    true
}

fn handle_thread_key(code: KeyCode, view: &mut ThreadViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let threads = snapshot.map(|snapshot| &snapshot.threads);
    let thread = threads.and_then(|threads| threads.threads.get(view.thread_selected));
    let operation = thread.and_then(|thread| thread.operations.get(view.operation_selected));
    let participant = operation.and_then(|operation| operation.participants.get(view.participant_selected));
    match code {
        KeyCode::Up => match view.focus {
            ThreadFocus::Threads => {
                view.thread_selected = view.thread_selected.saturating_sub(1);
                view.reset_operation();
            }

            ThreadFocus::Operations => {
                view.operation_selected = view.operation_selected.saturating_sub(1);
                view.reset_participant();
            }
            ThreadFocus::Participants => {
                view.participant_selected = view.participant_selected.saturating_sub(1);
                view.reset_object();
            }
            ThreadFocus::Objects => {
                view.object_selected = view.object_selected.saturating_sub(1);
                view.stack_scroll = 0;
            }
        },
        KeyCode::Down => match view.focus {
            ThreadFocus::Threads => {
                let count = threads.map_or(0, |threads| threads.threads.len());
                view.thread_selected = (view.thread_selected + 1).min(count.saturating_sub(1));
                view.reset_operation();
            }
            ThreadFocus::Operations => {
                let count = thread.map_or(0, |thread| thread.operations.len());
                view.operation_selected = (view.operation_selected + 1).min(count.saturating_sub(1));
                view.reset_participant();
            }
            ThreadFocus::Participants => {
                let count = operation.map_or(0, |operation| operation.participants.len());
                view.participant_selected = (view.participant_selected + 1).min(count.saturating_sub(1));
                view.reset_object();
            }
            ThreadFocus::Objects => {
                let count = participant.map_or(0, |participant| participant.objects.len());
                view.object_selected = (view.object_selected + 1).min(count.saturating_sub(1));
                view.stack_scroll = 0;
            }
        },
        KeyCode::Enter => {
            view.focus = match view.focus {
                ThreadFocus::Threads => ThreadFocus::Operations,
                ThreadFocus::Operations => ThreadFocus::Participants,
                ThreadFocus::Participants | ThreadFocus::Objects => ThreadFocus::Objects,
            };
        }
        KeyCode::Backspace => {
            view.focus = match view.focus {
                ThreadFocus::Threads => return false,
                ThreadFocus::Operations => ThreadFocus::Threads,
                ThreadFocus::Participants => ThreadFocus::Operations,
                ThreadFocus::Objects => ThreadFocus::Participants,
            };
        }
        KeyCode::PageUp => view.stack_scroll = view.stack_scroll.saturating_sub(1),
        KeyCode::PageDown => view.stack_scroll = view.stack_scroll.saturating_add(1),
        KeyCode::Char('f') => {
            view.stack_filter = view.stack_filter.toggle();
            view.stack_scroll = 0;
        }
        _ => return false,
    }
    true
}

fn handle_runtime_key(code: KeyCode, view: &mut RuntimeViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let runtime = snapshot.map(|snapshot| &snapshot.runtime);
    let worker = runtime.and_then(|runtime| runtime.workers.get(view.worker_selected));
    match code {
        KeyCode::Up => match view.focus {
            RuntimeFocus::Workers => {
                view.worker_selected = view.worker_selected.saturating_sub(1);
                view.reset_task();
            }
            RuntimeFocus::Tasks => {
                view.task_selected = view.task_selected.saturating_sub(1);
                view.detail_view = RuntimeDetailView::Details;
                view.detail_scroll = 0;
            }
            RuntimeFocus::Details => view.detail_scroll = view.detail_scroll.saturating_sub(1),
        },
        KeyCode::Down => match view.focus {
            RuntimeFocus::Workers => {
                let count = runtime.map_or(0, |runtime| runtime.workers.len());
                view.worker_selected = (view.worker_selected + 1).min(count.saturating_sub(1));
                view.reset_task();
            }
            RuntimeFocus::Tasks => {
                let count = worker.map_or(0, |worker| worker.tasks.len());
                view.task_selected = (view.task_selected + 1).min(count.saturating_sub(1));
                view.detail_view = RuntimeDetailView::Details;
                view.detail_scroll = 0;
            }
            RuntimeFocus::Details => view.detail_scroll = view.detail_scroll.saturating_add(1),
        },
        KeyCode::Enter => {
            view.focus = match view.focus {
                RuntimeFocus::Workers => RuntimeFocus::Tasks,
                RuntimeFocus::Tasks | RuntimeFocus::Details => RuntimeFocus::Details,
            };
        }
        KeyCode::Backspace => {
            view.focus = match view.focus {
                RuntimeFocus::Workers => return false,
                RuntimeFocus::Tasks => RuntimeFocus::Workers,
                RuntimeFocus::Details => RuntimeFocus::Tasks,
            };
        }
        KeyCode::Tab | KeyCode::BackTab if view.focus == RuntimeFocus::Details => {
            view.detail_view = view.detail_view.toggle();
            view.detail_scroll = 0;
        }
        KeyCode::PageUp => view.detail_scroll = view.detail_scroll.saturating_sub(5),
        KeyCode::PageDown => view.detail_scroll = view.detail_scroll.saturating_add(5),
        KeyCode::Char('[') => {
            view.task_sort = view.task_sort.previous();
            view.reset_task();
        }
        KeyCode::Char(']') => {
            view.task_sort = view.task_sort.next();
            view.reset_task();
        }
        KeyCode::Char('r') => {
            view.task_sort_descending = !view.task_sort_descending;
            view.reset_task();
        }
        _ => return false,
    }
    true
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn capture_connected_snapshot(
    descriptor: &MonitorDescriptor,
    options: SnapshotOptions,
    progress: &Sender<CaptureMessage>,
) -> Result<CaptureOutcome, String> {
    report_capture_step(progress, CaptureStep::Capture)?;
    let bytes = capture_snapshot(descriptor, options).map_err(|error| error.to_string())?;
    report_capture_step(progress, CaptureStep::Decode)?;
    let decoded = seismograph::snapshot::decode(&bytes).map_err(|error| format!("invalid Seismograph snapshot: {error}"))?;
    let allocator = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .ok_or(super::Error::MissingMemorySource)
        .and_then(|source| seismograph_rallocator::decode(&source.data).map_err(super::Error::MemorySnapshot));
    let runtime_source = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .and_then(|source| seismograph_runtime::snapshot::decode(&source.data).ok());
    let runtime_addresses = runtime_source
        .iter()
        .flat_map(|source| &source.addresses)
        .map(|lookup| {
            seismograph_rallocator::callers::AddressLookup::from_fields(seismograph_rallocator::callers::AddressLookupFields {
                address: lookup.address,
                symbol: lookup.symbol.clone(),
                filename: lookup.filename.clone(),
                line: lookup.line,
                column: lookup.column,
            })
        })
        .collect::<Vec<_>>();
    let (memory, allocations, runtime, heap_error, mut status) = match allocator {
        Ok(allocator) => {
            let mut addresses = allocator
                .addresses
                .iter()
                .cloned()
                .map(|lookup| (lookup.address, lookup))
                .collect::<std::collections::BTreeMap<_, _>>();
            for lookup in runtime_addresses {
                addresses.insert(lookup.address, lookup);
            }
            let addresses = addresses.into_values().collect::<Vec<_>>();
            let runtime = RuntimeSnapshot::from_events(&decoded, &addresses, runtime_source.as_ref());
            (
                Some(MemorySnapshot::from_snapshot(&allocator)),
                Some(AllocationSnapshot::from_snapshot(&allocator)),
                runtime,
                None,
                String::new(),
            )
        }
        Err(error) => {
            let error = format!("heap data unavailable: {error}");
            (
                None,
                None,
                RuntimeSnapshot::from_events(&decoded, &runtime_addresses, runtime_source.as_ref()),
                Some(error.clone()),
                error,
            )
        }
    };
    let snapshot = Box::new(CapturedSnapshot {
        memory,
        allocations,
        heap_error,
        primitives: runtime.primitives,
        runtime: runtime.runtime,
        threads: runtime.threads,
        captured_at: SystemTime::now(),
        captured_instant: Instant::now(),
    });
    report_capture_step(progress, CaptureStep::Save)?;
    if let Err(error) = save_snapshot(descriptor, &bytes) {
        status = error.to_string();
    }
    Ok(CaptureOutcome { snapshot, status })
}

fn report_capture_step(progress: &Sender<CaptureMessage>, step: CaptureStep) -> Result<(), String> {
    progress
        .send_sync(CaptureMessage::Progress(step))
        .map_err(|_send_error| "snapshot progress receiver closed".to_owned())
}

pub(super) fn format_sampling_percentage(sampling_one_in: u32) -> String {
    let percentage = 100.0 / f64::from(sampling_one_in.max(1));
    let precision = if percentage >= 10.0 {
        1
    } else if percentage >= 1.0 {
        2
    } else if percentage >= 0.1 {
        3
    } else if percentage >= 0.01 {
        4
    } else {
        6
    };
    let formatted = format!("{percentage:.precision$}");
    format!("{}%", formatted.trim_end_matches('0').trim_end_matches('.'))
}

const fn next_buffer_disposition(disposition: EventBufferDisposition) -> EventBufferDisposition {
    match disposition {
        EventBufferDisposition::Retain => EventBufferDisposition::Clear,
        EventBufferDisposition::Clear => EventBufferDisposition::Release,
        EventBufferDisposition::Release => EventBufferDisposition::Retain,
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::{PrimitiveSnapshot, RuntimeMonitorSnapshot, ThreadSnapshot};
    use super::*;

    fn descriptor(id: u8) -> MonitorDescriptor {
        MonitorDescriptor {
            name: format!("test-{id}"),
            instance: None,
            process_id: u32::from(id),
            instance_id: seismograph_protocol::monitor::InstanceId::from_bytes([id; 16]),
            port: 0,
            authentication: seismograph_protocol::monitor::AuthenticationToken::from_bytes([id; 32]),
        }
    }

    fn empty_capture() -> Box<CapturedSnapshot> {
        Box::new(CapturedSnapshot {
            memory: None,
            allocations: None,
            heap_error: None,
            primitives: PrimitiveSnapshot {
                total_events: 0,
                lost_events: 0,
                groups: Vec::new(),
            },
            runtime: RuntimeMonitorSnapshot::default(),
            threads: ThreadSnapshot { threads: Vec::new() },
            captured_at: SystemTime::UNIX_EPOCH,
            captured_instant: Instant::now(),
        })
    }

    fn connected_app(tab: MonitorTab) -> App {
        let mut app = App::new();
        app.screen = Screen::Connected {
            descriptor: descriptor(1),
            recording: RecordingConfiguration::default(),
            tab,
            snapshot: Some(empty_capture()),
        };
        app
    }

    fn connected_fields(screen: &Screen) -> Option<(seismograph_protocol::monitor::InstanceId, RecordingConfiguration, MonitorTab, bool)> {
        match screen {
            Screen::Browse => None,
            Screen::Connected {
                descriptor,
                recording,
                tab,
                snapshot,
            } => Some((descriptor.instance_id, *recording, *tab, snapshot.is_some())),
        }
    }

    #[test]
    fn recording_fields_have_stable_labels_and_values() {
        let mut configuration = RecordingConfiguration::default();
        configuration.allocations.enabled = true;
        configuration.allocations.capture_backtraces = true;
        configuration.allocations.sampling_one_in = 8;
        configuration.general_events.enabled = true;
        configuration.general_events.capture_backtraces = true;
        configuration.general_events.sampling_one_in = 20;
        configuration.arc_dereferences.enabled = true;
        configuration.arc_dereferences.capture_backtraces = true;
        configuration.arc_dereferences.sampling_one_in = 100;
        configuration.runtime_tasks.enabled = true;
        configuration.runtime_tasks.capture_backtraces = true;
        configuration.io.enabled = true;
        configuration.io.capture_backtraces = true;
        configuration.io.sampling_one_in = 4;
        configuration.cache.enabled = true;
        configuration.cache.capture_backtraces = true;
        configuration.cache.sampling_one_in = 10;
        configuration.event_capacity_per_thread = 1_024;

        assert_eq!(
            RecordingConfigurationField::ALL.map(|field| (field.label(), field.value(configuration))),
            [
                ("Allocations recording", "on".into()),
                ("Allocations backtraces", "on".into()),
                ("Allocations sampling", "1/8 (12.5%)".into()),
                ("General events recording", "on".into()),
                ("General events backtraces", "on".into()),
                ("General events sampling", "1/20 (5%)".into()),
                ("Arc dereference recording", "on".into()),
                ("Arc dereference backtraces", "on".into()),
                ("Arc dereference sampling", "1/100 (1%)".into()),
                ("Runtime task recording", "on".into()),
                ("Runtime task backtraces", "on".into()),
                ("I/O recording", "on".into()),
                ("I/O backtraces", "on".into()),
                ("I/O resource sampling", "1/4 (25%)".into()),
                ("Cache recording", "on".into()),
                ("Cache backtraces", "on".into()),
                ("Cache tier sampling", "1/10 (10%)".into()),
                ("Event buffer capacity", "1024 events / thread".into()),
                ("OK", String::new()),
                ("Cancel", String::new()),
            ]
        );
    }

    #[test]
    fn every_recording_field_adjusts_only_its_value() {
        let mut configuration = RecordingConfiguration::default();
        for field in RecordingConfigurationField::ALL {
            field.adjust(&mut configuration, 1);
        }

        assert_eq!(
            (
                configuration.allocations,
                configuration.general_events,
                configuration.arc_dereferences,
                configuration.runtime_tasks,
                configuration.io,
                configuration.cache,
                configuration.event_capacity_per_thread,
            ),
            (
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 2,
                },
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 2,
                },
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 2,
                },
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 1,
                },
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 2,
                },
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 2,
                },
                131_072,
            )
        );
    }

    #[test]
    fn view_state_resets_restore_navigation_origins() {
        let mut allocation = AllocationViewState::new();
        allocation.selected = 3;
        allocation.stack_scroll = 4;
        allocation.reset_position();
        let mut primitive = PrimitiveViewState::new();
        primitive.operation_selected = 3;
        primitive.hotspot_selected = 4;
        primitive.stack_scroll = 5;
        primitive.reset_operations();
        let mut heap = HeapViewState::new();
        heap.focus = HeapFocus::Hotspots;
        heap.bucket_selected = 3;
        heap.hotspot_selected = 4;
        heap.stack_scroll = 5;
        heap.reset();
        let mut thread = ThreadViewState::new();
        thread.focus = ThreadFocus::Objects;
        thread.thread_selected = 2;
        thread.operation_selected = 3;
        thread.participant_selected = 4;
        thread.object_selected = 5;
        thread.stack_scroll = 6;
        thread.reset();
        let mut runtime = RuntimeViewState::new();
        runtime.focus = RuntimeFocus::Details;
        runtime.worker_selected = 2;
        runtime.task_selected = 3;
        runtime.detail_view = RuntimeDetailView::SpawnStack;
        runtime.detail_scroll = 4;
        runtime.reset();

        assert_eq!(
            (allocation, primitive, heap, thread, runtime),
            (
                AllocationViewState::new(),
                PrimitiveViewState::new(),
                HeapViewState::new(),
                ThreadViewState::new(),
                RuntimeViewState::new(),
            )
        );
    }

    #[test]
    fn tabs_and_runtime_detail_views_cover_every_variant() {
        assert_eq!(
            [
                MonitorTab::Info,
                MonitorTab::Heaps,
                MonitorTab::Allocations,
                MonitorTab::Primitives,
                MonitorTab::Threads,
                MonitorTab::Runtime,
            ]
            .map(|tab| (tab.index(), tab.next(), tab.previous())),
            [
                (0, MonitorTab::Heaps, MonitorTab::Runtime),
                (1, MonitorTab::Allocations, MonitorTab::Info),
                (2, MonitorTab::Primitives, MonitorTab::Heaps),
                (3, MonitorTab::Threads, MonitorTab::Allocations),
                (4, MonitorTab::Runtime, MonitorTab::Primitives),
                (5, MonitorTab::Info, MonitorTab::Threads),
            ]
        );
        assert_eq!(
            [
                (RuntimeDetailView::Details.toggle(), RuntimeDetailView::Details.label()),
                (RuntimeDetailView::SpawnStack.toggle(), RuntimeDetailView::SpawnStack.label()),
            ],
            [
                (RuntimeDetailView::SpawnStack, "Details"),
                (RuntimeDetailView::Details, "Spawn Stack"),
            ]
        );
    }

    #[test]
    fn next_tab_wraps_to_info() {
        assert_eq!(MonitorTab::Runtime.next(), MonitorTab::Info);
    }

    #[test]
    fn previous_tab_wraps_to_runtime() {
        assert_eq!(MonitorTab::Info.previous(), MonitorTab::Runtime);
    }

    #[test]
    fn recording_toggle_preserves_backtraces() {
        let mut configuration = RecordingConfiguration::default();
        configuration.allocations.enabled = true;
        configuration.allocations.capture_backtraces = true;
        RecordingConfigurationField::AllocationRecording.adjust(&mut configuration, 1);

        assert_eq!(
            (configuration.allocations.enabled, configuration.allocations.capture_backtraces),
            (false, true)
        );
    }

    #[test]
    fn backtrace_toggle_preserves_recording() {
        let mut configuration = RecordingConfiguration::default();
        RecordingConfigurationField::GeneralBacktraces.adjust(&mut configuration, 1);

        assert_eq!(
            (
                configuration.general_events.enabled,
                configuration.general_events.capture_backtraces
            ),
            (false, true)
        );
    }

    #[test]
    fn configuration_choices_cover_protocol_bounds() {
        assert_eq!(
            (
                EVENT_BUFFER_CAPACITIES.first().copied(),
                EVENT_BUFFER_CAPACITIES.last().copied(),
                adjusted_value(1_000, &EVENT_BUFFER_CAPACITIES, 0),
                EVENT_SAMPLING_RATES.first().copied(),
                EVENT_SAMPLING_RATES.last().copied(),
                adjusted_value(1_000, &EVENT_SAMPLING_RATES, 0),
            ),
            (Some(64), Some(1_048_576), 1_024, Some(1), Some(65_536), 1_024)
        );
    }

    #[test]
    fn configuration_popup_uses_arrow_keys_and_escape() {
        let mut app = App::new();
        app.recording_configuration_popup = Some(RecordingConfigurationPopup {
            draft: RecordingConfiguration::default(),
            selected: 0,
        });
        app.handle_key(KeyCode::Down);
        let moved = app.recording_configuration_popup;
        app.handle_key(KeyCode::Esc);

        assert_eq!(
            (moved, app.recording_configuration_popup),
            (
                Some(RecordingConfigurationPopup {
                    draft: RecordingConfiguration::default(),
                    selected: 1,
                }),
                None
            )
        );
    }

    #[test]
    fn idle_polling_and_popup_keys_are_noops() {
        let mut app = App::new();
        assert!(connected_fields(&app.screen).is_none());
        let refresh = app.next_refresh();
        app.handle_recording_configuration_key(KeyCode::Char('x'));
        app.poll_snapshot_capture();
        app.poll_discovery();
        app.poll_recorder_statistics();
        app.recording_configuration_popup = Some(RecordingConfigurationPopup {
            draft: RecordingConfiguration::default(),
            selected: 0,
        });
        app.handle_recording_configuration_key(KeyCode::Char('x'));

        assert_eq!((app.next_refresh(), app.status), (refresh, String::new()));
    }

    #[test]
    fn configuration_popup_handles_adjust_cancel_and_disconnected_apply() {
        let mut app = App::new();
        app.recording_configuration_popup = Some(RecordingConfigurationPopup {
            draft: RecordingConfiguration::default(),
            selected: 0,
        });
        app.handle_key(KeyCode::Right);
        app.handle_key(KeyCode::Left);
        app.handle_key(KeyCode::Char(' '));
        app.handle_key(KeyCode::Up);
        app.handle_key(KeyCode::Down);
        app.recording_configuration_popup.as_mut().unwrap().selected = RecordingConfigurationField::ALL.len() - 1;
        app.handle_key(KeyCode::Enter);

        assert_eq!(
            (app.recording_configuration_popup, app.status.as_str()),
            (None, "Recording configuration unchanged")
        );

        app.recording_configuration_popup = Some(RecordingConfigurationPopup {
            draft: RecordingConfiguration::default(),
            selected: RecordingConfigurationField::ALL.len() - 2,
        });
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.recording_configuration_popup, None);
    }

    #[test]
    fn configuration_popup_opens_with_current_configuration() {
        let mut app = App::new();
        let mut recording = RecordingConfiguration::default();
        recording.allocations.enabled = true;
        app.screen = Screen::Connected {
            descriptor: MonitorDescriptor {
                name: "test".into(),
                instance: None,
                process_id: 1,
                instance_id: seismograph_protocol::monitor::InstanceId::from_bytes([1; 16]),
                port: 0,
                authentication: seismograph_protocol::monitor::AuthenticationToken::from_bytes([2; 32]),
            },
            recording,
            tab: MonitorTab::Info,
            snapshot: None,
        };

        app.handle_key(KeyCode::Char('c'));

        assert_eq!(
            app.recording_configuration_popup,
            Some(RecordingConfigurationPopup {
                draft: recording,
                selected: 0,
            })
        );
    }

    #[test]
    fn sampling_percentages_remain_readable_across_the_picker_range() {
        assert_eq!(
            (
                format_sampling_percentage(1),
                format_sampling_percentage(20),
                format_sampling_percentage(100),
                format_sampling_percentage(1_000),
                format_sampling_percentage(10_000),
                format_sampling_percentage(65_536),
            ),
            (
                "100%".into(),
                "5%".into(),
                "1%".into(),
                "0.1%".into(),
                "0.01%".into(),
                "0.001526%".into()
            )
        );
    }

    #[test]
    fn cancelling_configuration_popup_discards_draft_changes() {
        let mut app = App::new();
        app.screen = Screen::Connected {
            descriptor: MonitorDescriptor {
                name: "unreachable".into(),
                instance: None,
                process_id: 1,
                instance_id: seismograph_protocol::monitor::InstanceId::from_bytes([1; 16]),
                port: 0,
                authentication: seismograph_protocol::monitor::AuthenticationToken::from_bytes([2; 32]),
            },
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Info,
            snapshot: None,
        };
        app.recording_configuration_popup = Some(RecordingConfigurationPopup {
            draft: RecordingConfiguration::default(),
            selected: 0,
        });

        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Esc);

        assert_eq!(
            (connected_fields(&app.screen).unwrap().1, app.recording_configuration_popup),
            (RecordingConfiguration::default(), None)
        );
    }

    #[test]
    fn snapshot_buffer_disposition_cycles() {
        assert_eq!(
            (
                next_buffer_disposition(EventBufferDisposition::Retain),
                next_buffer_disposition(EventBufferDisposition::Clear),
                next_buffer_disposition(EventBufferDisposition::Release),
            ),
            (
                EventBufferDisposition::Clear,
                EventBufferDisposition::Release,
                EventBufferDisposition::Retain,
            )
        );
    }

    #[test]
    fn browser_and_connected_keys_update_screen_state() {
        let mut app = App::new();
        app.instances = vec![
            Instance {
                descriptor: descriptor(1),
                recording: RecordingConfiguration::default(),
            },
            Instance {
                descriptor: descriptor(2),
                recording: RecordingConfiguration::default(),
            },
        ];
        assert!(app.handle_key(KeyCode::Char('q')));
        assert!(app.handle_key(KeyCode::Char('Q')));
        assert!(!app.handle_key(KeyCode::Char('x')));
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Up);
        app.handle_key(KeyCode::Enter);
        assert_eq!(connected_fields(&app.screen).unwrap().0, descriptor(1).instance_id);

        for (key, expected) in [
            (KeyCode::Char('2'), MonitorTab::Heaps),
            (KeyCode::Char('3'), MonitorTab::Allocations),
            (KeyCode::Char('4'), MonitorTab::Primitives),
            (KeyCode::Char('5'), MonitorTab::Threads),
            (KeyCode::Char('6'), MonitorTab::Runtime),
            (KeyCode::Char('1'), MonitorTab::Info),
            (KeyCode::Left, MonitorTab::Runtime),
            (KeyCode::Right, MonitorTab::Info),
            (KeyCode::Char('h'), MonitorTab::Runtime),
            (KeyCode::Char('l'), MonitorTab::Info),
            (KeyCode::BackTab, MonitorTab::Runtime),
            (KeyCode::Tab, MonitorTab::Info),
        ] {
            app.handle_key(key);
            assert_eq!(connected_fields(&app.screen).unwrap().2, expected);
        }
        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.snapshot_options.event_buffers, EventBufferDisposition::Clear);
        app.handle_key(KeyCode::Char('x'));
        app.handle_key(KeyCode::Esc);
        assert!(matches!(app.screen, Screen::Browse));
        app.handle_key(KeyCode::Char('r'));
    }

    #[test]
    fn capture_key_is_ignored_while_capture_is_running_or_browsing() {
        let mut app = App::new();
        app.handle_key(KeyCode::Char('s'));
        let (_sender, receiver) = unbounded();
        app.capture_receiver = Some(receiver);
        app.screen = Screen::Connected {
            descriptor: descriptor(1),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Info,
            snapshot: None,
        };
        app.handle_key(KeyCode::Char('s'));

        assert!(app.capture_receiver.is_some());
    }

    #[test]
    fn connected_capture_key_starts_background_capture() {
        let mut app = connected_app(MonitorTab::Info);

        app.handle_key(KeyCode::Char('s'));

        assert!(app.capture_receiver.is_some());
    }

    #[test]
    fn allocation_sort_key_moves_to_next_column() {
        let mut view = AllocationViewState::new();
        handle_allocation_key(KeyCode::Char(']'), &mut view, None);

        assert_eq!(
            view,
            AllocationViewState {
                sort: AllocationSort::AllocatedBytes,
                descending: true,
                selected: 0,
                stack_scroll: 0,
                stack_filter: AllocationStackFilter::Application,
            }
        );
    }

    #[test]
    fn allocation_filter_key_shows_all_frames() {
        let mut view = AllocationViewState::new();
        handle_allocation_key(KeyCode::Char('f'), &mut view, None);

        assert_eq!(view.stack_filter, AllocationStackFilter::All);
    }

    #[test]
    fn allocation_keys_cover_navigation_sorting_and_scroll() {
        let mut view = AllocationViewState::new();
        view.selected = 2;
        view.stack_scroll = 2;
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Char('['),
            KeyCode::Char('r'),
        ] {
            assert!(handle_allocation_key(key, &mut view, None));
        }
        assert!(!handle_allocation_key(KeyCode::Char('x'), &mut view, None));
    }

    #[test]
    fn heap_keys_switch_tiers_and_focus_hotspots() {
        let mut view = HeapViewState::new();
        handle_heap_key(KeyCode::Char(']'), &mut view, None);
        handle_heap_key(KeyCode::Enter, &mut view, None);

        assert_eq!((view.tier, view.focus), (MemoryTier::Medium, HeapFocus::Hotspots));
    }

    #[test]
    fn heap_keys_cover_both_focus_levels() {
        let mut view = HeapViewState::new();
        view.bucket_selected = 2;
        view.stack_scroll = 2;
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char('['),
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Char('f'),
            KeyCode::Backspace,
        ] {
            assert!(handle_heap_key(key, &mut view, None));
        }
        assert!(!handle_heap_key(KeyCode::Backspace, &mut view, None));
        assert!(!handle_heap_key(KeyCode::Char('x'), &mut view, None));
    }

    #[test]
    fn activity_rate_uses_event_delta_over_elapsed_time() {
        assert_eq!(activity_rate(1_000, 2_500, Duration::from_millis(500)), 3_000);
    }

    #[test]
    fn activity_rate_handles_zero_time_counter_reset_and_overflow() {
        assert_eq!(
            (
                activity_rate(10, 20, Duration::from_nanos(1)),
                activity_rate(20, 10, Duration::from_secs(1)),
                activity_rate(0, u64::MAX, Duration::from_millis(1)),
            ),
            (0, 0, u64::MAX)
        );
    }

    #[test]
    fn first_activity_observation_only_establishes_the_baseline() {
        let mut app = App::new();
        app.record_activity(recorder_statistics_with_total(10));

        assert_eq!(app.activity_samples, VecDeque::new());
    }

    #[test]
    fn zero_activity_after_the_baseline_is_retained() {
        let mut app = App::new();
        app.record_activity(recorder_statistics_with_total(10));
        app.record_activity(recorder_statistics_with_total(10));

        assert_eq!(
            app.activity_samples
                .iter()
                .map(|sample| sample.events_per_second)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }

    #[test]
    fn activity_history_is_bounded() {
        let mut app = App::new();
        for total in 0..=MAX_ACTIVITY_SAMPLES as u64 + 1 {
            app.record_activity(recorder_statistics_with_total(total));
        }

        assert_eq!(app.activity_samples.len(), MAX_ACTIVITY_SAMPLES);
    }

    #[test]
    fn capture_steps_advance_phase_progress() {
        assert_eq!(
            CaptureStep::ALL.map(|step| (step.index(), step.label(), step.progress())),
            [
                (0, "Capture process snapshot", 1.0 / 6.0),
                (1, "Decode telemetry", 1.0 / 2.0),
                (2, "Save snapshot file", 5.0 / 6.0),
            ]
        );
    }

    #[test]
    fn failed_snapshot_capture_is_available_to_content_panels() {
        let mut app = App::new();

        app.finish_snapshot_capture(Err("snapshot decode failed".into()));

        assert_eq!(app.snapshot_error.as_deref(), Some("snapshot decode failed"));
    }

    #[test]
    fn successful_snapshot_capture_resets_views_and_installs_snapshot() {
        let mut app = connected_app(MonitorTab::Info);
        app.heap_view.focus = HeapFocus::Hotspots;
        app.allocation_view.selected = 3;
        app.thread_view.focus = ThreadFocus::Objects;
        app.runtime_view.focus = RuntimeFocus::Details;
        app.finish_snapshot_capture(Ok(CaptureOutcome {
            snapshot: empty_capture(),
            status: "saved".into(),
        }));

        assert_eq!(
            (
                connected_fields(&app.screen).unwrap().3,
                app.heap_view,
                app.allocation_view,
                app.thread_view,
                app.runtime_view,
                app.status.as_str(),
            ),
            (
                true,
                HeapViewState::new(),
                AllocationViewState::new(),
                ThreadViewState::new(),
                RuntimeViewState::new(),
                "saved",
            )
        );
    }

    #[test]
    fn snapshot_capture_messages_cover_progress_completion_and_closed_worker() {
        let mut app = App::new();
        let (sender, receiver) = unbounded();
        app.capture_receiver = Some(receiver);
        sender.send_sync(CaptureMessage::Progress(CaptureStep::Decode)).unwrap();
        sender.send_sync(CaptureMessage::Complete(Err("decode failed".into()))).unwrap();
        app.poll_snapshot_capture();
        assert_eq!((app.capture_step, app.snapshot_error.as_deref()), (None, Some("decode failed")));

        let (sender, receiver) = unbounded::<CaptureMessage>();
        drop(sender);
        app.capture_receiver = Some(receiver);
        app.poll_snapshot_capture();
        assert_eq!(app.snapshot_error.as_deref(), Some("Snapshot capture worker stopped unexpectedly"));
    }

    #[test]
    fn discovery_messages_preserve_selection_and_handle_errors() {
        let mut app = App::new();
        app.instances = vec![Instance {
            descriptor: descriptor(2),
            recording: RecordingConfiguration::default(),
        }];
        let (sender, receiver) = unbounded();
        sender
            .send_sync(Ok(vec![
                Instance {
                    descriptor: descriptor(1),
                    recording: RecordingConfiguration::default(),
                },
                Instance {
                    descriptor: descriptor(2),
                    recording: RecordingConfiguration::default(),
                },
            ]))
            .unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();
        assert_eq!((app.selected, app.status.as_str()), (1, "2 application(s) available"));

        let (sender, receiver) = unbounded();
        sender.send_sync(Ok(Vec::new())).unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();
        assert_eq!(app.status, "No reachable Seismograph monitors found");

        let (sender, receiver) = unbounded();
        sender.send_sync(Err("discovery failed".into())).unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();
        assert_eq!(app.status, "discovery failed");

        let (sender, receiver) = unbounded::<Result<Vec<Instance>, String>>();
        drop(sender);
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();
        assert_eq!(app.status, "Monitor discovery worker stopped unexpectedly");
    }

    #[test]
    fn discovery_results_are_ignored_after_connecting() {
        let mut app = connected_app(MonitorTab::Info);
        let (sender, receiver) = unbounded();
        sender.send_sync(Ok(Vec::new())).unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();

        assert!(matches!(app.screen, Screen::Connected { .. }));
    }

    #[test]
    fn statistics_messages_record_activity_and_handle_errors() {
        let mut app = App::new();
        let (sender, receiver) = unbounded();
        sender.send_sync(Ok(recorder_statistics_with_total(10))).unwrap();
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert_eq!(app.recorder_statistics.as_ref().map(|value| value.total_events), Some(10));

        let (sender, receiver) = unbounded();
        sender.send_sync(Err("statistics failed".into())).unwrap();
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert_eq!(app.status, "statistics failed");

        let (sender, receiver) = unbounded::<Result<RecorderStatistics, String>>();
        drop(sender);
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert_eq!(app.status, "Recorder statistics worker stopped unexpectedly");
    }

    #[test]
    fn snapshot_capture_does_not_block_tab_navigation() {
        let mut app = App::new();
        let (_sender, receiver) = unbounded();
        app.capture_receiver = Some(receiver);
        app.screen = Screen::Connected {
            descriptor: MonitorDescriptor {
                name: "test".into(),
                instance: None,
                process_id: 1,
                instance_id: seismograph_protocol::monitor::InstanceId::from_bytes([1; 16]),
                port: 1,
                authentication: seismograph_protocol::monitor::AuthenticationToken::from_bytes([2; 32]),
            },
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Heaps,
            snapshot: None,
        };

        app.handle_key(KeyCode::Right);

        assert_eq!(connected_fields(&app.screen).unwrap().2, MonitorTab::Allocations);
    }

    #[test]
    fn primitive_enter_and_backspace_move_between_levels() {
        let mut view = PrimitiveViewState::new();
        handle_primitive_key(KeyCode::Enter, &mut view, None);
        handle_primitive_key(KeyCode::Enter, &mut view, None);
        handle_primitive_key(KeyCode::Backspace, &mut view, None);

        assert_eq!(view.focus, PrimitiveFocus::Operations);
    }

    #[test]
    fn primitive_keys_cover_navigation_sorting_filtering_and_scroll() {
        let mut view = PrimitiveViewState::new();
        view.primitive_selected = 2;
        view.operation_selected = 2;
        view.hotspot_selected = 2;
        view.stack_scroll = 2;
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Char('['),
            KeyCode::Char(']'),
            KeyCode::Char('r'),
            KeyCode::Char('f'),
            KeyCode::Backspace,
            KeyCode::Backspace,
        ] {
            assert!(handle_primitive_key(key, &mut view, None));
        }
        assert!(!handle_primitive_key(KeyCode::Backspace, &mut view, None));
        assert!(!handle_primitive_key(KeyCode::Char('x'), &mut view, None));
    }

    #[test]
    fn runtime_details_tab_toggles_spawn_stack_view() {
        let mut view = RuntimeViewState::new();
        view.focus = RuntimeFocus::Details;

        handle_runtime_key(KeyCode::Tab, &mut view, None);
        let stack_view = view.detail_view;
        handle_runtime_key(KeyCode::BackTab, &mut view, None);

        assert_eq!(
            (stack_view, view.detail_view),
            (RuntimeDetailView::SpawnStack, RuntimeDetailView::Details)
        );
    }

    #[test]
    fn runtime_keys_cover_navigation_sorting_and_scroll() {
        let mut view = RuntimeViewState::new();
        view.worker_selected = 2;
        view.task_selected = 2;
        view.detail_scroll = 6;
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Char('['),
            KeyCode::Char(']'),
            KeyCode::Char('r'),
            KeyCode::Backspace,
            KeyCode::Backspace,
        ] {
            assert!(handle_runtime_key(key, &mut view, None));
        }
        assert!(!handle_runtime_key(KeyCode::Backspace, &mut view, None));
        assert!(!handle_runtime_key(KeyCode::Char('x'), &mut view, None));
    }

    fn recorder_statistics_with_total(total_events: u64) -> RecorderStatistics {
        RecorderStatistics {
            thread_count: 1,
            total_events,
            retained_events: total_events,
            lost_events: 0,
            event_capacity_per_thread: 65_536,
            allocated_bytes: 18 * 1024 * 1024,
            recording: RecordingConfiguration::default(),
        }
    }

    #[test]
    fn thread_enter_and_backspace_move_between_levels() {
        let mut view = ThreadViewState::new();
        handle_thread_key(KeyCode::Enter, &mut view, None);
        handle_thread_key(KeyCode::Enter, &mut view, None);
        handle_thread_key(KeyCode::Enter, &mut view, None);
        handle_thread_key(KeyCode::Backspace, &mut view, None);

        assert_eq!(view.focus, ThreadFocus::Participants);
    }

    #[test]
    fn thread_keys_cover_navigation_filtering_and_scroll() {
        let mut view = ThreadViewState::new();
        view.thread_selected = 2;
        view.operation_selected = 2;
        view.participant_selected = 2;
        view.object_selected = 2;
        view.stack_scroll = 2;
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Char('f'),
            KeyCode::Backspace,
            KeyCode::Backspace,
            KeyCode::Backspace,
        ] {
            assert!(handle_thread_key(key, &mut view, None));
        }
        assert!(!handle_thread_key(KeyCode::Backspace, &mut view, None));
        assert!(!handle_thread_key(KeyCode::Char('x'), &mut view, None));
    }

    #[test]
    fn progress_reporting_succeeds_and_reports_closed_receiver() {
        let (sender, receiver) = unbounded();
        report_capture_step(&sender, CaptureStep::Save).unwrap();
        assert!(matches!(receiver.recv_sync().unwrap(), CaptureMessage::Progress(CaptureStep::Save)));
        drop(receiver);
        assert_eq!(
            report_capture_step(&sender, CaptureStep::Save),
            Err("snapshot progress receiver closed".into())
        );
    }
}

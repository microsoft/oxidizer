// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::KeyCode;
use performables::sync::channel::{Receiver, Sender, unbounded};
use seismograph_protocol::message::{RecorderStatistics, RecordingConfiguration, SnapshotOptions};
use seismograph_protocol::monitor::{InstanceId, MonitorDescriptor};

use super::client::{
    capture_snapshot, capture_snapshot_and_stop, clear_event_buffers, discover, handshake, recorder_activity, save_snapshot, set_recording,
};
use super::data::{AllocationSort, AllocationStackFilter, CapturedSnapshot, MemoryTier, MemoryTierData, PrimitiveSort, RuntimeTaskSort};
use super::live_activity::{LiveActivity, RecorderUpdate};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const MAX_ACTIVITY_SAMPLES: usize = 120;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum CaptureMode {
    #[default]
    Continue,
    Stop,
}

impl CaptureMode {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Continue => "Record and continue",
            Self::Stop => "Record and stop",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Continue => Self::Stop,
            Self::Stop => Self::Continue,
        }
    }
}
pub(super) const EVENT_BUFFER_CAPACITIES: [u32; 15] = [
    64, 128, 256, 512, 1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576,
];
pub(super) const EVENT_SAMPLING_RATES: [u32; 19] = [
    1, 2, 4, 8, 16, 20, 32, 64, 100, 128, 256, 512, 1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536,
];

pub(super) struct Instance {
    pub(super) descriptor: MonitorDescriptor,
    pub(super) recording: RecordingConfiguration,
    pub(super) server_version: Option<String>,
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
    modes: [RecordingMode; 6],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordingMode {
    Off,
    On,
    Custom,
}

impl RecordingMode {
    const ALL: [Self; 3] = [Self::Off, Self::On, Self::Custom];

    const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Custom => "custom",
        }
    }

    fn adjusted(self, direction: isize) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[index.saturating_add_signed(direction).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecorderKind {
    Allocations,
    General,
    ArcDereferences,
    RuntimeTasks,
    Io,
    Cache,
}

impl RecorderKind {
    const ALL: [Self; 6] = [
        Self::Allocations,
        Self::General,
        Self::ArcDereferences,
        Self::RuntimeTasks,
        Self::Io,
        Self::Cache,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Allocations => 0,
            Self::General => 1,
            Self::ArcDereferences => 2,
            Self::RuntimeTasks => 3,
            Self::Io => 4,
            Self::Cache => 5,
        }
    }
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
}

impl RecordingConfigurationField {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::AllocationRecording => "Allocations",
            Self::GeneralRecording => "Events",
            Self::ArcDereferenceRecording => "Arc",
            Self::RuntimeTaskRecording => "Runtime tasks",
            Self::IoRecording => "I/O",
            Self::CacheRecording => "Cache",
            Self::AllocationBacktraces
            | Self::GeneralBacktraces
            | Self::ArcDereferenceBacktraces
            | Self::RuntimeTaskBacktraces
            | Self::IoBacktraces
            | Self::CacheBacktraces => "  Backtraces",
            Self::AllocationSampling | Self::GeneralSampling | Self::ArcDereferenceSampling | Self::IoSampling | Self::CacheSampling => {
                "  Sampling"
            }
            Self::EventBufferCapacity => "Event buffer capacity",
        }
    }

    const fn recorder(self) -> Option<RecorderKind> {
        match self {
            Self::AllocationRecording | Self::AllocationBacktraces | Self::AllocationSampling => Some(RecorderKind::Allocations),
            Self::GeneralRecording | Self::GeneralBacktraces | Self::GeneralSampling => Some(RecorderKind::General),
            Self::ArcDereferenceRecording | Self::ArcDereferenceBacktraces | Self::ArcDereferenceSampling => {
                Some(RecorderKind::ArcDereferences)
            }
            Self::RuntimeTaskRecording | Self::RuntimeTaskBacktraces => Some(RecorderKind::RuntimeTasks),
            Self::IoRecording | Self::IoBacktraces | Self::IoSampling => Some(RecorderKind::Io),
            Self::CacheRecording | Self::CacheBacktraces | Self::CacheSampling => Some(RecorderKind::Cache),
            Self::EventBufferCapacity => None,
        }
    }

    pub(super) fn value(self, popup: RecordingConfigurationPopup) -> String {
        let configuration = popup.draft;
        match self {
            Self::AllocationBacktraces => toggle_label(configuration.allocations.capture_backtraces),
            Self::AllocationSampling => sampling_label(configuration.allocations.sampling_one_in),
            Self::GeneralBacktraces => toggle_label(configuration.general_events.capture_backtraces),
            Self::GeneralSampling => sampling_label(configuration.general_events.sampling_one_in),
            Self::ArcDereferenceBacktraces => toggle_label(configuration.arc_dereferences.capture_backtraces),
            Self::ArcDereferenceSampling => sampling_label(configuration.arc_dereferences.sampling_one_in),
            Self::RuntimeTaskBacktraces => toggle_label(configuration.runtime_tasks.capture_backtraces),
            Self::IoBacktraces => toggle_label(configuration.io.capture_backtraces),
            Self::IoSampling => sampling_label(configuration.io.sampling_one_in),
            Self::CacheBacktraces => toggle_label(configuration.cache.capture_backtraces),
            Self::CacheSampling => sampling_label(configuration.cache.sampling_one_in),
            Self::EventBufferCapacity => format!("{} events / thread", configuration.event_capacity_per_thread),
            Self::AllocationRecording
            | Self::GeneralRecording
            | Self::ArcDereferenceRecording
            | Self::RuntimeTaskRecording
            | Self::IoRecording
            | Self::CacheRecording => popup
                .mode(self.recorder().expect("recording mode fields always identify a recorder"))
                .label()
                .to_owned(),
        }
    }

    fn adjust(self, popup: &mut RecordingConfigurationPopup, direction: isize) {
        let configuration = &mut popup.draft;
        match self {
            Self::AllocationBacktraces => {
                configuration.allocations.capture_backtraces = !configuration.allocations.capture_backtraces;
            }
            Self::AllocationSampling => {
                configuration.allocations.sampling_one_in =
                    adjusted_value(configuration.allocations.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::GeneralBacktraces => {
                configuration.general_events.capture_backtraces = !configuration.general_events.capture_backtraces;
            }
            Self::GeneralSampling => {
                configuration.general_events.sampling_one_in =
                    adjusted_value(configuration.general_events.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::ArcDereferenceBacktraces => {
                configuration.arc_dereferences.capture_backtraces = !configuration.arc_dereferences.capture_backtraces;
            }
            Self::ArcDereferenceSampling => {
                configuration.arc_dereferences.sampling_one_in =
                    adjusted_value(configuration.arc_dereferences.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
            Self::RuntimeTaskBacktraces => {
                configuration.runtime_tasks.capture_backtraces = !configuration.runtime_tasks.capture_backtraces;
            }
            Self::IoBacktraces => {
                configuration.io.capture_backtraces = !configuration.io.capture_backtraces;
            }
            Self::IoSampling => {
                configuration.io.sampling_one_in = adjusted_value(configuration.io.sampling_one_in, &EVENT_SAMPLING_RATES, direction);
            }
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
            Self::AllocationRecording
            | Self::GeneralRecording
            | Self::ArcDereferenceRecording
            | Self::RuntimeTaskRecording
            | Self::IoRecording
            | Self::CacheRecording => {
                let recorder = self.recorder().expect("recording mode fields always identify a recorder");
                popup.modes[recorder.index()] = popup.mode(recorder).adjusted(direction);
            }
        }
    }
}

impl RecordingConfigurationPopup {
    pub(super) fn new(draft: RecordingConfiguration) -> Self {
        let modes = RecorderKind::ALL.map(|recorder| recording_mode(recorder_policy(draft, recorder)));
        Self { draft, selected: 0, modes }
    }

    pub(super) fn field(self) -> RecordingConfigurationField {
        self.fields()[self.selected]
    }

    pub(super) fn fields(self) -> Vec<RecordingConfigurationField> {
        let mut fields = Vec::with_capacity(19);
        for (recorder, mode, group) in [
            (
                RecorderKind::Allocations,
                self.mode(RecorderKind::Allocations),
                [
                    RecordingConfigurationField::AllocationRecording,
                    RecordingConfigurationField::AllocationBacktraces,
                    RecordingConfigurationField::AllocationSampling,
                ]
                .as_slice(),
            ),
            (
                RecorderKind::General,
                self.mode(RecorderKind::General),
                [
                    RecordingConfigurationField::GeneralRecording,
                    RecordingConfigurationField::GeneralBacktraces,
                    RecordingConfigurationField::GeneralSampling,
                ]
                .as_slice(),
            ),
            (
                RecorderKind::ArcDereferences,
                self.mode(RecorderKind::ArcDereferences),
                [
                    RecordingConfigurationField::ArcDereferenceRecording,
                    RecordingConfigurationField::ArcDereferenceBacktraces,
                    RecordingConfigurationField::ArcDereferenceSampling,
                ]
                .as_slice(),
            ),
            (
                RecorderKind::RuntimeTasks,
                self.mode(RecorderKind::RuntimeTasks),
                [
                    RecordingConfigurationField::RuntimeTaskRecording,
                    RecordingConfigurationField::RuntimeTaskBacktraces,
                    RecordingConfigurationField::RuntimeTaskBacktraces,
                ]
                .as_slice(),
            ),
            (
                RecorderKind::Io,
                self.mode(RecorderKind::Io),
                [
                    RecordingConfigurationField::IoRecording,
                    RecordingConfigurationField::IoBacktraces,
                    RecordingConfigurationField::IoSampling,
                ]
                .as_slice(),
            ),
            (
                RecorderKind::Cache,
                self.mode(RecorderKind::Cache),
                [
                    RecordingConfigurationField::CacheRecording,
                    RecordingConfigurationField::CacheBacktraces,
                    RecordingConfigurationField::CacheSampling,
                ]
                .as_slice(),
            ),
        ] {
            fields.push(group[0]);
            if mode == RecordingMode::Custom {
                fields.push(group[1]);
                if recorder != RecorderKind::RuntimeTasks {
                    fields.push(group[2]);
                }
            }
        }
        fields.push(RecordingConfigurationField::EventBufferCapacity);
        fields
    }

    fn mode(self, recorder: RecorderKind) -> RecordingMode {
        self.modes[recorder.index()]
    }

    fn configuration(self) -> RecordingConfiguration {
        let mut configuration = self.draft;
        for recorder in RecorderKind::ALL {
            let policy = recorder_policy_mut(&mut configuration, recorder);
            match self.mode(recorder) {
                RecordingMode::Off => policy.enabled = false,
                RecordingMode::On => {
                    policy.enabled = true;
                    policy.capture_backtraces = true;
                    policy.sampling_one_in = 1;
                }
                RecordingMode::Custom => policy.enabled = true,
            }
        }
        configuration
    }
}

fn recorder_policy(configuration: RecordingConfiguration, recorder: RecorderKind) -> seismograph_protocol::message::RecordingPolicy {
    match recorder {
        RecorderKind::Allocations => configuration.allocations,
        RecorderKind::General => configuration.general_events,
        RecorderKind::ArcDereferences => configuration.arc_dereferences,
        RecorderKind::RuntimeTasks => configuration.runtime_tasks,
        RecorderKind::Io => configuration.io,
        RecorderKind::Cache => configuration.cache,
    }
}

fn recorder_policy_mut(
    configuration: &mut RecordingConfiguration,
    recorder: RecorderKind,
) -> &mut seismograph_protocol::message::RecordingPolicy {
    match recorder {
        RecorderKind::Allocations => &mut configuration.allocations,
        RecorderKind::General => &mut configuration.general_events,
        RecorderKind::ArcDereferences => &mut configuration.arc_dereferences,
        RecorderKind::RuntimeTasks => &mut configuration.runtime_tasks,
        RecorderKind::Io => &mut configuration.io,
        RecorderKind::Cache => &mut configuration.cache,
    }
}

const fn recording_mode(policy: seismograph_protocol::message::RecordingPolicy) -> RecordingMode {
    if !policy.enabled {
        RecordingMode::Off
    } else if !policy.capture_backtraces || policy.sampling_one_in != 1 {
        RecordingMode::Custom
    } else {
        RecordingMode::On
    }
}

pub(super) fn recording_policy_label(policy: seismograph_protocol::message::RecordingPolicy) -> &'static str {
    recording_mode(policy).label()
}

fn toggle_label(enabled: bool) -> String {
    if enabled { "on" } else { "off" }.to_owned()
}

fn sampling_label(sampling_one_in: u32) -> String {
    format!("1/{sampling_one_in} ({})", format_sampling_percentage(sampling_one_in))
}

fn adjusted_value(current: u32, values: &[u32], direction: isize) -> u32 {
    let last_index = values.len().saturating_sub(1);
    let index = values.iter().position(|candidate| *candidate >= current).unwrap_or(last_index);
    let adjusted = index.saturating_add_signed(direction).min(last_index);
    values[adjusted]
}

fn advance_selection(selected: usize, item_count: usize) -> usize {
    selected.saturating_add(1).min(item_count.saturating_sub(1))
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
    pub(super) activity_scroll: usize,
    pub(super) task_sort: RuntimeTaskSort,
    pub(super) task_sort_descending: bool,
    pub(super) task_histogram: TaskHistogram,
    pub(super) events: TaskEventsViewState,
}

/// Fits readable duration buckets in the activity panes while keeping keyboard bounds stable.
pub(super) const RUNTIME_HISTOGRAM_BINS: usize = super::runtime_timeline::HISTOGRAM_BINS;

impl RuntimeViewState {
    const fn new() -> Self {
        Self {
            focus: RuntimeFocus::Workers,
            worker_selected: 0,
            task_selected: 0,
            activity_scroll: 0,
            task_sort: RuntimeTaskSort::Polls,
            task_sort_descending: true,
            task_histogram: TaskHistogram::Poll,
            events: TaskEventsViewState::new(),
        }
    }

    fn reset(&mut self) {
        self.focus = RuntimeFocus::Workers;
        self.worker_selected = 0;
        self.reset_task();
    }

    fn reset_task(&mut self) {
        self.task_selected = 0;
        self.reset_details();
    }

    fn reset_details(&mut self) {
        self.activity_scroll = 0;
        self.events.reset();
    }

    pub(super) fn selected_task<'a>(
        &self,
        runtime: &'a super::data::RuntimeMonitorSnapshot,
    ) -> Option<&'a super::data::RuntimeTaskSummary> {
        let worker = runtime
            .workers
            .get(self.worker_selected.min(runtime.workers.len().saturating_sub(1)))?;
        let tasks = worker.sorted_tasks(self.task_sort, self.task_sort_descending);
        tasks.get(self.task_selected.min(tasks.len().saturating_sub(1))).copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TaskHistogram {
    Poll,
    Ready,
}

impl TaskHistogram {
    const fn toggle(self) -> Self {
        match self {
            Self::Poll => Self::Ready,
            Self::Ready => Self::Poll,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TaskEventsViewState {
    pub(super) focus: TaskEventsFocus,
    pub(super) operation_selected: usize,
    pub(super) event_selected: usize,
    pub(super) stack_scroll: usize,
    pub(super) stack_horizontal_scroll: u16,
    pub(super) stack_filter: AllocationStackFilter,
}

impl TaskEventsViewState {
    const fn new() -> Self {
        Self {
            focus: TaskEventsFocus::Operations,
            operation_selected: 0,
            event_selected: 0,
            stack_scroll: 0,
            stack_horizontal_scroll: 0,
            stack_filter: AllocationStackFilter::Application,
        }
    }

    fn reset(&mut self) {
        self.focus = TaskEventsFocus::Operations;
        self.operation_selected = 0;
        self.reset_event();
    }

    fn reset_event(&mut self) {
        self.event_selected = 0;
        self.stack_scroll = 0;
        self.stack_horizontal_scroll = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TaskEventsFocus {
    Operations,
    Occurrences,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeFocus {
    Workers,
    Tasks,
    Activity,
    Events,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct IoViewState {
    pub(super) focus: IoFocus,
    pub(super) resource_selected: usize,
    pub(super) operation_selected: usize,
}

impl IoViewState {
    const fn new() -> Self {
        Self {
            focus: IoFocus::Resources,
            resource_selected: 0,
            operation_selected: 0,
        }
    }

    fn reset(&mut self) {
        self.focus = IoFocus::Resources;
        self.resource_selected = 0;
        self.operation_selected = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IoFocus {
    Resources,
    Operations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CacheViewState {
    pub(super) focus: CacheFocus,
    pub(super) tier_selected: usize,
    pub(super) operation_selected: usize,
}

impl CacheViewState {
    const fn new() -> Self {
        Self {
            focus: CacheFocus::Tiers,
            tier_selected: 0,
            operation_selected: 0,
        }
    }

    fn reset(&mut self) {
        self.focus = CacheFocus::Tiers;
        self.tier_selected = 0;
        self.operation_selected = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CacheFocus {
    Tiers,
    Operations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MonitorTab {
    Info,
    Heaps,
    Allocations,
    Primitives,
    Threads,
    Runtime,
    Io,
    Cache,
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
            Self::Io => 6,
            Self::Cache => 7,
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Info => Self::Heaps,
            Self::Heaps => Self::Allocations,
            Self::Allocations => Self::Primitives,
            Self::Primitives => Self::Threads,
            Self::Threads => Self::Runtime,
            Self::Runtime => Self::Io,
            Self::Io => Self::Cache,
            Self::Cache => Self::Info,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Info => Self::Cache,
            Self::Heaps => Self::Info,
            Self::Allocations => Self::Heaps,
            Self::Primitives => Self::Allocations,
            Self::Threads => Self::Primitives,
            Self::Runtime => Self::Threads,
            Self::Io => Self::Runtime,
            Self::Cache => Self::Io,
        }
    }
}

pub(super) enum Screen {
    Browse,
    Offline {
        path: PathBuf,
        tab: MonitorTab,
        snapshot: Option<Box<CapturedSnapshot>>,
    },
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
    pub(super) server_version: Option<String>,
    pub(super) status: String,
    pub(super) allocation_view: AllocationViewState,
    pub(super) heap_view: HeapViewState,
    pub(super) primitive_view: PrimitiveViewState,
    pub(super) thread_view: ThreadViewState,
    pub(super) runtime_view: RuntimeViewState,
    pub(super) io_view: IoViewState,
    pub(super) cache_view: CacheViewState,
    pub(super) panels: super::panels::Panels,
    pub(super) activity_samples: VecDeque<ActivitySample>,
    pub(super) recorder_statistics: Option<RecorderStatistics>,
    pub(super) live_activity: LiveActivity,
    pub(super) info_thread_selected: usize,
    pub(super) capture_mode: CaptureMode,
    pub(super) recording_unknown: bool,
    pub(super) snapshot_error: Option<String>,
    pub(super) recording_configuration_popup: Option<RecordingConfigurationPopup>,
    pub(super) filters: super::filter_ui::Filters,
    pub(super) help: Option<super::help::Help>,
    activity_observed_at: Option<Instant>,
    pub(super) capture_started_at: Option<Instant>,
    pub(super) capture_step: Option<CaptureStep>,
    capture_instance_id: Option<InstanceId>,
    capture_generation: u64,
    clear_in_progress: bool,
    capture_receiver: Option<Receiver<CaptureMessage>>,
    discovery_receiver: Option<Receiver<Result<Vec<Instance>, String>>>,
    statistics_receiver: Option<Receiver<Result<RecorderUpdate, String>>>,
    recording_receiver: Option<Receiver<Result<RecordingUpdate, String>>>,
    connection_generation: u64,
    recording_generation: u64,
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
    Recording(Result<RecordingConfiguration, String>),
    Cleared(Result<(), String>),
    Complete(Result<CaptureOutcome, String>),
}

struct CaptureOutcome {
    snapshot: Box<CapturedSnapshot>,
    status: String,
}

struct RecordingUpdate {
    descriptor: MonitorDescriptor,
    configuration: RecordingConfiguration,
}

impl App {
    pub(super) fn offline(path: PathBuf) -> Self {
        Self {
            screen: Screen::Offline {
                path,
                tab: MonitorTab::Info,
                snapshot: None,
            },
            status: "Loading snapshot…".into(),
            ..Self::new()
        }
    }

    pub(super) fn finish_offline_load(&mut self, loaded: Box<CapturedSnapshot>) {
        if let Screen::Offline { snapshot, .. } = &mut self.screen {
            self.status = loaded.heap_error.clone().unwrap_or_default();
            *snapshot = Some(loaded);
            self.filter_snapshot_arrived();
        }
    }

    pub(super) fn new() -> Self {
        Self {
            instances: Vec::new(),
            selected: 0,
            screen: Screen::Browse,
            server_version: None,
            status: String::new(),
            allocation_view: AllocationViewState::new(),
            heap_view: HeapViewState::new(),
            primitive_view: PrimitiveViewState::new(),
            thread_view: ThreadViewState::new(),
            runtime_view: RuntimeViewState::new(),
            io_view: IoViewState::new(),
            cache_view: CacheViewState::new(),
            panels: super::panels::Panels::default(),
            activity_samples: VecDeque::new(),
            recorder_statistics: None,
            live_activity: LiveActivity::default(),
            info_thread_selected: 0,
            capture_mode: CaptureMode::default(),
            recording_unknown: false,
            snapshot_error: None,
            recording_configuration_popup: None,
            filters: super::filter_ui::Filters::default(),
            help: None,
            activity_observed_at: None,
            capture_started_at: None,
            capture_step: None,
            capture_instance_id: None,
            capture_generation: 0,
            clear_in_progress: false,
            capture_receiver: None,
            discovery_receiver: None,
            statistics_receiver: None,
            recording_receiver: None,
            connection_generation: 0,
            recording_generation: 0,
            next_refresh: Instant::now(),
        }
    }

    pub(super) const fn next_refresh(&self) -> Instant {
        self.next_refresh
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(super) fn refresh(&mut self) {
        self.refresh_with(discover, recorder_activity);
    }

    fn refresh_with<D, S>(&mut self, discover: D, recorder_statistics: S)
    where
        D: FnOnce() -> Result<Vec<Instance>, super::Error> + Send + 'static,
        S: FnOnce(&MonitorDescriptor) -> Result<RecorderUpdate, super::Error> + Send + 'static,
    {
        self.next_refresh = Instant::now().checked_add(REFRESH_INTERVAL).unwrap_or_else(Instant::now);
        if matches!(self.screen, Screen::Offline { .. }) {
            return;
        }
        if let Screen::Connected { descriptor, .. } = &self.screen {
            if self.statistics_receiver.is_none() && self.recording_receiver.is_none() && self.capture_receiver.is_none() {
                self.start_recorder_statistics_with(descriptor.clone(), recorder_statistics);
            }
            return;
        }
        if self.discovery_receiver.is_none() {
            self.start_discovery_with(discover);
        }
    }

    pub(super) fn handle_key(&mut self, code: KeyCode) -> bool {
        if self.handle_help_key(code) {
            return false;
        }
        if self.filters.popup.is_some() {
            self.handle_filter_key(code);
            return false;
        }
        if matches!(code, KeyCode::Char('q' | 'Q')) {
            return true;
        }
        let offline = matches!(self.screen, Screen::Offline { .. });
        if offline && code == KeyCode::Esc {
            return true;
        }
        if offline && matches!(code, KeyCode::Char('s' | 'c' | 'd' | 'C')) {
            return false;
        }
        let capture_in_progress = self.capture_receiver.is_some();
        if self.recording_configuration_popup.is_some() {
            self.handle_recording_configuration_key(code);
            return false;
        }
        if code == KeyCode::Char('F') {
            self.open_filter_popup();
            return false;
        }
        if matches!(code, KeyCode::Char('s' | 'C')) {
            if capture_in_progress || self.recording_receiver.is_some() {
                return false;
            }
            let capture = match &self.screen {
                Screen::Connected { descriptor, .. } => Some(descriptor.clone()),
                Screen::Browse | Screen::Offline { .. } => None,
            };
            if let Some(descriptor) = capture {
                if code == KeyCode::Char('C') {
                    self.start_clear_buffers(descriptor);
                } else {
                    self.start_snapshot_capture(descriptor, self.capture_mode);
                }
            }
            return false;
        }
        match &mut self.screen {
            Screen::Browse => match code {
                KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                KeyCode::Down => {
                    self.selected = advance_selection(self.selected, self.instances.len());
                }
                KeyCode::Enter => self.connect_selected_instance(),
                KeyCode::Char('r') => self.refresh(),
                _ => {}
            },
            Screen::Connected { tab, snapshot, .. } | Screen::Offline { tab, snapshot, .. } => {
                let handled_by_tab = match *tab {
                    MonitorTab::Heaps => handle_heap_key(code, &mut self.heap_view, snapshot.as_deref()),
                    MonitorTab::Allocations => handle_allocation_key(code, &mut self.allocation_view, snapshot.as_deref()),
                    MonitorTab::Primitives => handle_primitive_key(code, &mut self.primitive_view, snapshot.as_deref()),
                    MonitorTab::Threads => handle_thread_key(code, &mut self.thread_view, snapshot.as_deref()),
                    MonitorTab::Runtime => handle_runtime_key(code, &mut self.runtime_view, snapshot.as_deref()),
                    MonitorTab::Io => handle_io_key(code, &mut self.io_view, snapshot.as_deref()),
                    MonitorTab::Cache => handle_cache_key(code, &mut self.cache_view, snapshot.as_deref()),
                    MonitorTab::Info => handle_info_key(code, &mut self.info_thread_selected, self.live_activity.threads.len()),
                };
                if handled_by_tab {
                    return false;
                }
                match code {
                    KeyCode::Esc => {
                        self.filters.invalidate();
                        self.connection_generation += 1;
                        self.statistics_receiver = None;
                        self.screen = Screen::Browse;
                        self.server_version = None;
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
                    KeyCode::Char('7') => *tab = MonitorTab::Io,
                    KeyCode::Char('8') => *tab = MonitorTab::Cache,
                    KeyCode::Char('d') => {
                        self.capture_mode = self.capture_mode.next();
                        self.status = format!("{} selected (s to capture)", self.capture_mode.label());
                    }
                    KeyCode::Char('c') if self.recording_receiver.is_none() && !capture_in_progress && !self.recording_unknown => {
                        if let Screen::Connected { recording, .. } = &self.screen {
                            self.recording_configuration_popup = Some(RecordingConfigurationPopup::new(*recording));
                        }
                    }
                    _ => {}
                }
            }
        }
        false
    }

    fn connect_selected_instance(&mut self) {
        if let Some(instance) = self.instances.get(self.selected) {
            self.filters.invalidate();
            self.connection_generation += 1;
            self.statistics_receiver = None;
            self.discovery_receiver = None;
            self.next_refresh = Instant::now();
            self.activity_samples.clear();
            self.recorder_statistics = None;
            self.live_activity = LiveActivity::default();
            self.info_thread_selected = 0;
            self.activity_observed_at = None;
            self.recording_unknown = false;
            self.server_version.clone_from(&instance.server_version);
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

    fn handle_recording_configuration_key(&mut self, code: KeyCode) {
        if code == KeyCode::Enter {
            if let Some(popup) = self.recording_configuration_popup {
                self.apply_recording_configuration(popup.configuration());
            }
            return;
        }
        let Some(popup) = &mut self.recording_configuration_popup else {
            return;
        };
        match code {
            KeyCode::Up => popup.selected = popup.selected.saturating_sub(1),
            KeyCode::Down => {
                popup.selected = advance_selection(popup.selected, popup.fields().len());
            }
            KeyCode::Left => {
                popup.field().adjust(popup, -1);
                popup.selected = popup.selected.min(popup.fields().len().saturating_sub(1));
            }
            KeyCode::Right | KeyCode::Char(' ') => {
                popup.field().adjust(popup, 1);
                popup.selected = popup.selected.min(popup.fields().len().saturating_sub(1));
            }
            KeyCode::Esc => self.recording_configuration_popup = None,
            _ => {}
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn apply_recording_configuration(&mut self, configuration: RecordingConfiguration) {
        self.apply_recording_configuration_with(configuration, set_recording);
    }

    fn apply_recording_configuration_with<F>(&mut self, configuration: RecordingConfiguration, set_recording: F)
    where
        F: FnOnce(&MonitorDescriptor, RecordingConfiguration) -> Result<RecordingConfiguration, super::Error> + Send + 'static,
    {
        if self.recording_receiver.is_some() || self.capture_receiver.is_some() || self.recording_unknown {
            return;
        }
        let Screen::Connected { descriptor, .. } = &self.screen else {
            self.recording_configuration_popup = None;
            return;
        };
        let descriptor = descriptor.clone();
        self.recording_configuration_popup = None;
        self.start_recording_configuration_with(descriptor, configuration, set_recording);
    }

    pub(super) fn poll_recording_configuration(&mut self) {
        let Some(result) = receive_worker_result(self.recording_receiver.as_ref(), "Recording configuration") else {
            return;
        };
        self.recording_receiver = None;
        // A failed multi-command update can still have changed part of the configuration.
        self.next_refresh = Instant::now();
        if self.recording_generation != self.connection_generation {
            return;
        }
        match result {
            Ok(update) => {
                if let Screen::Connected { descriptor, recording, .. } = &mut self.screen
                    && descriptor.instance_id == update.descriptor.instance_id
                {
                    *recording = update.configuration;
                    if let Some(statistics) = &mut self.recorder_statistics {
                        statistics.recording = update.configuration;
                        statistics.event_capacity_per_thread = u64::from(update.configuration.event_capacity_per_thread);
                    }
                }
                self.status = "Recording configuration applied".into();
            }
            Err(error) => self.status = error,
        }
    }

    pub(super) fn poll_snapshot_capture(&mut self) {
        loop {
            let message = match receive_capture_message(self.capture_receiver.as_ref()) {
                Ok(Some(message)) => message,
                Ok(None) => return,
                Err(error) => {
                    if self.clear_in_progress {
                        self.finish_clear_buffers(Err(error));
                    } else {
                        self.finish_snapshot_capture(Err(error));
                    }
                    return;
                }
            };
            match message {
                CaptureMessage::Recording(result) => {
                    if self.capture_is_current() {
                        self.reconcile_recording(result);
                    }
                }
                CaptureMessage::Cleared(result) => {
                    self.finish_clear_buffers(result);
                    return;
                }
                CaptureMessage::Progress(step) => {
                    if self.capture_is_current() {
                        self.capture_step = Some(step);
                    }
                }
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
        if !matches!(self.screen, Screen::Connected { .. }) || self.recording_receiver.is_some() || self.capture_receiver.is_some() {
            return;
        }
        match result {
            Ok(update) => {
                let statistics = update.statistics();
                self.recording_unknown = false;
                if let Screen::Connected { recording, .. } = &mut self.screen {
                    *recording = statistics.recording;
                }
                // The popup is a draft, not another copy of the live configuration.
                self.record_recorder_update(update);
            }
            Err(error) => {
                self.live_activity.stale = true;
                self.status = error;
            }
        }
    }

    fn finish_snapshot_capture(&mut self, result: Result<CaptureOutcome, String>) {
        let current = self.capture_is_current();
        self.capture_receiver = None;
        self.capture_started_at = None;
        self.capture_step = None;
        let capture_instance_id = self.capture_instance_id.take();
        if !current {
            return;
        }
        match result {
            Ok(outcome) => {
                self.snapshot_error = None;
                self.status = outcome.status;
                if let Screen::Connected { descriptor, snapshot, .. } = &mut self.screen
                    && capture_instance_id == Some(descriptor.instance_id)
                {
                    *snapshot = Some(outcome.snapshot);
                    self.heap_view.reset();
                    self.allocation_view.reset_position();
                    self.thread_view.reset();
                    self.runtime_view.reset();
                    self.io_view.reset();
                    self.cache_view.reset();
                    self.filter_snapshot_arrived();
                }
            }

            Err(error) => {
                self.snapshot_error = Some(error.clone());
                self.status = error;
            }
        }
    }

    fn finish_clear_buffers(&mut self, result: Result<(), String>) {
        if self.capture_is_current() {
            self.status = match result {
                Ok(()) => "Server buffers cleared; recording policy, captured view and saved files preserved".into(),
                Err(error) => format!("Clear failed: {error}"),
            };
        }
        self.capture_receiver = None;
        self.capture_instance_id = None;
        self.clear_in_progress = false;
    }

    pub(super) fn reset_filtered_views(&mut self) {
        self.heap_view.reset();
        self.allocation_view.reset_position();
        self.primitive_view.focus = PrimitiveFocus::Types;
        self.primitive_view.primitive_selected = 0;
        self.primitive_view.reset_operations();
        self.thread_view.reset();
        self.runtime_view.reset();
        self.io_view.reset();
        self.cache_view.reset();
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_snapshot_capture(&mut self, descriptor: MonitorDescriptor, mode: CaptureMode) {
        let (sender, receiver) = unbounded();
        let instance_id = descriptor.instance_id;
        match thread::Builder::new().name("seismograph-snapshot".into()).spawn(move || {
            let result = capture_connected_snapshot(&descriptor, mode, &sender);
            let _receiver_closed = sender.send(CaptureMessage::Complete(result));
        }) {
            Ok(_worker) => {
                self.snapshot_error = None;
                self.capture_started_at = Some(Instant::now());
                self.capture_step = Some(CaptureStep::Capture);
                self.capture_instance_id = Some(instance_id);
                self.capture_generation = self.connection_generation;
                self.statistics_receiver = None;
                self.capture_receiver = Some(receiver);
                if mode == CaptureMode::Stop {
                    self.recording_unknown = true;
                }
                self.status.clear();
            }
            Err(error) => self.status = format!("failed to start snapshot capture: {error}"),
        }
    }

    fn capture_is_current(&self) -> bool {
        self.capture_generation == self.connection_generation
            && matches!(&self.screen, Screen::Connected { descriptor, .. }
                        if Some(descriptor.instance_id) == self.capture_instance_id)
    }

    fn reconcile_recording(&mut self, result: Result<RecordingConfiguration, String>) {
        self.statistics_receiver = None;
        self.activity_samples.clear();
        self.activity_observed_at = None;
        self.recorder_statistics = None;
        self.live_activity = LiveActivity::default();
        self.info_thread_selected = 0;
        match result {
            Ok(configuration) => {
                self.recording_unknown = false;
                if let Screen::Connected { recording, .. } = &mut self.screen {
                    *recording = configuration;
                }
            }
            Err(error) => {
                self.recording_unknown = true;
                self.status = format!("Recording state unknown; refresh required: {error}");
            }
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_clear_buffers(&mut self, descriptor: MonitorDescriptor) {
        let (sender, receiver) = unbounded();
        let instance_id = descriptor.instance_id;
        match thread::Builder::new().name("seismograph-clear".into()).spawn(move || {
            let result = clear_event_buffers(&descriptor).map_err(|error| error.to_string());
            let recording = handshake(&descriptor).map_err(|error| error.to_string());
            if sender.send(CaptureMessage::Recording(recording)).is_ok() {
                let _receiver_closed = sender.send(CaptureMessage::Cleared(result));
            }
        }) {
            Ok(_worker) => {
                self.capture_generation = self.connection_generation;
                self.capture_instance_id = Some(instance_id);
                self.capture_receiver = Some(receiver);
                self.statistics_receiver = None;
                self.clear_in_progress = true;
                self.status = "Clearing server buffers (no snapshot)...".into();
            }
            Err(error) => self.status = format!("failed to start clear worker: {error}"),
        }
    }
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_discovery_with<F>(&mut self, discover: F)
    where
        F: FnOnce() -> Result<Vec<Instance>, super::Error> + Send + 'static,
    {
        let (sender, receiver) = unbounded();
        match thread::Builder::new().name("seismograph-discovery".into()).spawn(move || {
            let result = discover().map_err(|error| error.to_string());
            let _receiver_closed = sender.send(result);
        }) {
            Ok(_worker) => self.discovery_receiver = Some(receiver),
            Err(error) => self.status = format!("failed to start monitor discovery worker: {error}"),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_recorder_statistics_with<F>(&mut self, descriptor: MonitorDescriptor, recorder_statistics: F)
    where
        F: FnOnce(&MonitorDescriptor) -> Result<RecorderUpdate, super::Error> + Send + 'static,
    {
        let (sender, receiver) = unbounded();
        match thread::Builder::new().name("seismograph-statistics".into()).spawn(move || {
            let result = recorder_statistics(&descriptor).map_err(|error| error.to_string());
            let _receiver_closed = sender.send(result);
        }) {
            Ok(_worker) => self.statistics_receiver = Some(receiver),
            Err(error) => self.status = format!("failed to start recorder statistics worker: {error}"),
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn start_recording_configuration_with<F>(
        &mut self,
        descriptor: MonitorDescriptor,
        configuration: RecordingConfiguration,
        set_recording: F,
    ) where
        F: FnOnce(&MonitorDescriptor, RecordingConfiguration) -> Result<RecordingConfiguration, super::Error> + Send + 'static,
    {
        let (sender, receiver) = unbounded();
        match thread::Builder::new().name("seismograph-recording".into()).spawn(move || {
            let result = set_recording(&descriptor, configuration)
                .map(|configuration| RecordingUpdate { descriptor, configuration })
                .map_err(|error| error.to_string());
            let _receiver_closed = sender.send(result);
        }) {
            Ok(_worker) => self.finish_start_recording_configuration(receiver, Ok(())),
            Err(error) => self.finish_start_recording_configuration(receiver, Err(error)),
        }
    }

    fn finish_start_recording_configuration(&mut self, receiver: Receiver<Result<RecordingUpdate, String>>, result: std::io::Result<()>) {
        match result {
            Ok(()) => {
                // Dropping this receiver invalidates reads started before this write.
                self.statistics_receiver = None;
                self.discovery_receiver = None;
                self.recording_generation = self.connection_generation;
                self.recording_receiver = Some(receiver);
                self.status = "Applying recording configuration...".into();
            }
            Err(error) => self.status = format!("failed to start recording configuration worker: {error}"),
        }
    }

    fn record_recorder_update(&mut self, update: RecorderUpdate) {
        let statistics = update.statistics();
        let selected = self
            .live_activity
            .threads
            .get(self.info_thread_selected)
            .map(|thread| thread.statistics.thread_id);
        match update {
            RecorderUpdate::Detailed(activity) => {
                if self.live_activity.record(activity, Instant::now()) {
                    self.activity_samples.clear();
                    self.recorder_statistics = None;
                    self.activity_observed_at = None;
                }
            }
            RecorderUpdate::Legacy(_) => self.live_activity.unsupported(),
        }
        self.info_thread_selected = selected
            .and_then(|id| {
                self.live_activity
                    .threads
                    .iter()
                    .position(|thread| thread.statistics.thread_id == id)
            })
            .unwrap_or_else(|| self.info_thread_selected.min(self.live_activity.threads.len().saturating_sub(1)));
        self.record_activity(statistics);
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
        let excess = self.activity_samples.len().saturating_sub(MAX_ACTIVITY_SAMPLES);
        drop(self.activity_samples.drain(..excess));
        self.activity_observed_at = Some(captured_at);
        self.recorder_statistics = Some(statistics);
    }
}

fn handle_info_key(code: KeyCode, selected: &mut usize, count: usize) -> bool {
    let last = count.saturating_sub(1);
    match code {
        KeyCode::Up => *selected = selected.saturating_sub(1).min(last),
        KeyCode::Down => *selected = selected.saturating_add(1).min(last),
        KeyCode::PageUp => *selected = selected.saturating_sub(8).min(last),
        KeyCode::PageDown => *selected = selected.saturating_add(8).min(last),
        KeyCode::Home => *selected = 0,
        KeyCode::End => *selected = last,
        _ => return false,
    }
    true
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn receive_capture_message(receiver: Option<&Receiver<CaptureMessage>>) -> Result<Option<CaptureMessage>, String> {
    match receiver.map(Receiver::try_recv) {
        Some(Ok(message)) => Ok(Some(message)),
        Some(Err(error)) if error.is_empty() => Ok(None),
        None => Ok(None),
        Some(Err(_)) => Err("Snapshot capture worker stopped unexpectedly".into()),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn receive_worker_result<T>(receiver: Option<&Receiver<Result<T, String>>>, worker: &str) -> Option<Result<T, String>> {
    match receiver.map(Receiver::try_recv) {
        Some(Ok(result)) => Some(result),
        Some(Err(error)) if error.is_empty() => None,
        None => None,
        Some(Err(_)) => Some(Err(format!("{worker} worker stopped unexpectedly"))),
    }
}

#[cfg(test)]
const fn workers_are_idle(capturing: bool, fetching_statistics: bool, updating_recording: bool) -> bool {
    !capturing && !fetching_statistics && !updating_recording
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
            view.selected = advance_selection(view.selected, hotspot_count);
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

fn tier_with_kind(tiers: &[MemoryTierData], kind: MemoryTier) -> Option<&MemoryTierData> {
    tiers.iter().find(|tier| tier.kind == kind)
}

fn handle_heap_key(code: KeyCode, view: &mut HeapViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let memory = snapshot.and_then(|snapshot| snapshot.memory.as_ref());
    let tier = memory.and_then(|memory| tier_with_kind(&memory.tiers, view.tier));
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
                view.bucket_selected = advance_selection(view.bucket_selected, count);
                view.reset_hotspot();
            }
            HeapFocus::Hotspots => {
                let count = bucket.map_or(0, |bucket| bucket.hotspots.len());
                view.hotspot_selected = advance_selection(view.hotspot_selected, count);
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
                view.primitive_selected = advance_selection(view.primitive_selected, count);
                view.reset_operations();
            }
            PrimitiveFocus::Operations => {
                let count = operations.as_ref().map_or(0, Vec::len);
                view.operation_selected = advance_selection(view.operation_selected, count);
                view.reset_hotspots();
            }
            PrimitiveFocus::Hotspots => {
                let count = operation.map_or(0, |operation| operation.hotspots.len());
                view.hotspot_selected = advance_selection(view.hotspot_selected, count);
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
                view.thread_selected = advance_selection(view.thread_selected, count);
                view.reset_operation();
            }
            ThreadFocus::Operations => {
                let count = thread.map_or(0, |thread| thread.operations.len());
                view.operation_selected = advance_selection(view.operation_selected, count);
                view.reset_participant();
            }
            ThreadFocus::Participants => {
                let count = operation.map_or(0, |operation| operation.participants.len());
                view.participant_selected = advance_selection(view.participant_selected, count);
                view.reset_object();
            }
            ThreadFocus::Objects => {
                let count = participant.map_or(0, |participant| participant.objects.len());
                view.object_selected = advance_selection(view.object_selected, count);
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
    if code == KeyCode::Char('e') {
        view.focus = RuntimeFocus::Events;
        return true;
    }
    if code == KeyCode::Char('t') {
        view.task_histogram = view.task_histogram.toggle();
        view.focus = RuntimeFocus::Activity;
        return true;
    }
    if view.focus == RuntimeFocus::Events {
        let task = runtime.and_then(|runtime| view.selected_task(runtime));
        let events = snapshot.and_then(|snapshot| task.and_then(|task| snapshot.task_events.tasks.get(&(task.runtime_id, task.task_id))));
        if code == KeyCode::Backspace && view.events.focus == TaskEventsFocus::Operations {
            view.focus = RuntimeFocus::Activity;
            return true;
        }
        return handle_task_events_key(code, &mut view.events, events);
    }
    match code {
        KeyCode::Left if view.focus == RuntimeFocus::Activity => view.activity_scroll = view.activity_scroll.saturating_sub(1),
        KeyCode::Right if view.focus == RuntimeFocus::Activity => {
            view.activity_scroll = view.activity_scroll.saturating_add(1).min(RUNTIME_HISTOGRAM_BINS - 1);
        }
        KeyCode::Up => match view.focus {
            RuntimeFocus::Workers => {
                view.worker_selected = view.worker_selected.saturating_sub(1);
                view.reset_task();
            }
            RuntimeFocus::Tasks => {
                view.task_selected = view.task_selected.saturating_sub(1);
                view.reset_details();
            }
            RuntimeFocus::Activity => view.activity_scroll = view.activity_scroll.saturating_sub(1),
            RuntimeFocus::Events => unreachable!("event focus is handled above"),
        },
        KeyCode::Down => match view.focus {
            RuntimeFocus::Workers => {
                let count = runtime.map_or(0, |runtime| runtime.workers.len());
                view.worker_selected = advance_selection(view.worker_selected, count);
                view.reset_task();
            }
            RuntimeFocus::Tasks => {
                let count = worker.map_or(0, |worker| worker.tasks.len());
                view.task_selected = advance_selection(view.task_selected, count);
                view.reset_details();
            }
            RuntimeFocus::Activity => view.activity_scroll = view.activity_scroll.saturating_add(1).min(RUNTIME_HISTOGRAM_BINS - 1),
            RuntimeFocus::Events => unreachable!("event focus is handled above"),
        },
        KeyCode::Enter => {
            view.focus = match view.focus {
                RuntimeFocus::Workers => RuntimeFocus::Tasks,
                RuntimeFocus::Tasks => RuntimeFocus::Activity,
                RuntimeFocus::Activity | RuntimeFocus::Events => RuntimeFocus::Events,
            };
        }
        KeyCode::Backspace => {
            view.focus = match view.focus {
                RuntimeFocus::Workers => return false,
                RuntimeFocus::Tasks => RuntimeFocus::Workers,
                RuntimeFocus::Activity => RuntimeFocus::Tasks,
                RuntimeFocus::Events => unreachable!("event focus is handled above"),
            };
        }
        KeyCode::PageUp if view.focus == RuntimeFocus::Activity => view.activity_scroll = view.activity_scroll.saturating_sub(5),
        KeyCode::PageDown if view.focus == RuntimeFocus::Activity => {
            view.activity_scroll = view.activity_scroll.saturating_add(5).min(RUNTIME_HISTOGRAM_BINS - 1);
        }
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

fn handle_task_events_key(code: KeyCode, view: &mut TaskEventsViewState, task: Option<&super::task_events::TaskEventSummary>) -> bool {
    let operation = task.and_then(|task| {
        task.operations
            .get(view.operation_selected.min(task.operations.len().saturating_sub(1)))
    });
    match code {
        KeyCode::Up | KeyCode::Down => {
            let next = |selected, count| {
                if code == KeyCode::Up {
                    usize::saturating_sub(selected, 1)
                } else {
                    advance_selection(selected, count)
                }
            };
            match view.focus {
                TaskEventsFocus::Operations => {
                    view.operation_selected = next(view.operation_selected, task.map_or(0, |task| task.operations.len()));
                    view.reset_event();
                }
                TaskEventsFocus::Occurrences => {
                    view.event_selected = next(view.event_selected, operation.map_or(0, |operation| operation.occurrences().len()));
                    view.stack_scroll = 0;
                    view.stack_horizontal_scroll = 0;
                }
            }
        }
        KeyCode::Enter => {
            view.focus = TaskEventsFocus::Occurrences;
        }
        KeyCode::Backspace => {
            view.focus = match view.focus {
                TaskEventsFocus::Operations => return false,
                TaskEventsFocus::Occurrences => TaskEventsFocus::Operations,
            };
        }
        KeyCode::PageUp => view.stack_scroll = view.stack_scroll.saturating_sub(5),
        KeyCode::PageDown => view.stack_scroll = view.stack_scroll.saturating_add(5),
        KeyCode::Left => view.stack_horizontal_scroll = view.stack_horizontal_scroll.saturating_sub(8),
        KeyCode::Right => view.stack_horizontal_scroll = view.stack_horizontal_scroll.saturating_add(8),
        KeyCode::Char('f') => {
            view.stack_filter = view.stack_filter.toggle();
            view.stack_scroll = 0;
            view.stack_horizontal_scroll = 0;
        }
        _ => return false,
    }
    true
}

fn handle_io_key(code: KeyCode, view: &mut IoViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let io = snapshot.map(|snapshot| &snapshot.io);
    let resource = io.and_then(|io| io.resources.get(view.resource_selected));
    match code {
        KeyCode::Up => match view.focus {
            IoFocus::Resources => {
                view.resource_selected = view.resource_selected.saturating_sub(1);
                view.operation_selected = 0;
            }
            IoFocus::Operations => view.operation_selected = view.operation_selected.saturating_sub(1),
        },
        KeyCode::Down => match view.focus {
            IoFocus::Resources => {
                let count = io.map_or(0, |io| io.resources.len());
                view.resource_selected = advance_selection(view.resource_selected, count);
                view.operation_selected = 0;
            }
            IoFocus::Operations => {
                let count = resource.map_or(0, |resource| resource.operations.len());
                view.operation_selected = advance_selection(view.operation_selected, count);
            }
        },
        KeyCode::Enter => view.focus = IoFocus::Operations,
        KeyCode::Backspace => {
            if view.focus == IoFocus::Resources {
                return false;
            }
            view.focus = IoFocus::Resources;
        }
        _ => return false,
    }
    true
}

fn handle_cache_key(code: KeyCode, view: &mut CacheViewState, snapshot: Option<&CapturedSnapshot>) -> bool {
    let cache = snapshot.map(|snapshot| &snapshot.cache);
    let tier = cache.and_then(|cache| cache.tiers.get(view.tier_selected));
    match code {
        KeyCode::Up => match view.focus {
            CacheFocus::Tiers => {
                view.tier_selected = view.tier_selected.saturating_sub(1);
                view.operation_selected = 0;
            }
            CacheFocus::Operations => view.operation_selected = view.operation_selected.saturating_sub(1),
        },
        KeyCode::Down => match view.focus {
            CacheFocus::Tiers => {
                let count = cache.map_or(0, |cache| cache.tiers.len());
                view.tier_selected = advance_selection(view.tier_selected, count);
                view.operation_selected = 0;
            }
            CacheFocus::Operations => {
                let count = tier.map_or(0, |tier| tier.operations.len());
                view.operation_selected = advance_selection(view.operation_selected, count);
            }
        },
        KeyCode::Enter => view.focus = CacheFocus::Operations,
        KeyCode::Backspace => {
            if view.focus == CacheFocus::Tiers {
                return false;
            }
            view.focus = CacheFocus::Tiers;
        }
        _ => return false,
    }
    true
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn capture_connected_snapshot(
    descriptor: &MonitorDescriptor,
    mode: CaptureMode,
    progress: &Sender<CaptureMessage>,
) -> Result<CaptureOutcome, String> {
    report_capture_step(progress, CaptureStep::Capture)?;
    let captured = match mode {
        CaptureMode::Continue => capture_snapshot(descriptor, SnapshotOptions::default()),
        CaptureMode::Stop => {
            let result = capture_snapshot_and_stop(descriptor);
            // Cleanup can succeed even when subsequent source capture/encoding fails.
            // Always reconcile, including after a lost response or remote error.
            let recording = handshake(descriptor).map_err(|error| error.to_string());
            progress
                .send(CaptureMessage::Recording(recording))
                .map_err(|_send_error| "snapshot progress receiver closed".to_owned())?;
            result
        }
    };
    let bytes = captured.map_err(|error| error.to_string())?;
    report_capture_step(progress, CaptureStep::Decode)?;
    let decoded = seismograph::snapshot::decode(&bytes).map_err(|error| format!("invalid Seismograph snapshot: {error}"))?;
    let mut snapshot = super::snapshot::prepare(decoded)?;
    snapshot.captured_at = Some(SystemTime::now());
    snapshot.captured_instant = Some(Instant::now());
    let mut status = snapshot.heap_error.clone().unwrap_or_else(|| match mode {
        CaptureMode::Continue => "Captured; server buffers and recording policy preserved".into(),
        CaptureMode::Stop => "Captured; recording stopped and server ring buffers deallocated".into(),
    });
    report_capture_step(progress, CaptureStep::Save)?;
    if let Err(error) = save_snapshot(descriptor, &bytes) {
        status = error.to_string();
    }
    Ok(CaptureOutcome { snapshot, status })
}

fn report_capture_step(progress: &Sender<CaptureMessage>, step: CaptureStep) -> Result<(), String> {
    progress
        .send(CaptureMessage::Progress(step))
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

#[cfg(test)]
mod tests {
    use super::super::data::{CacheMonitorSnapshot, IoMonitorSnapshot, PrimitiveSnapshot, RuntimeMonitorSnapshot, ThreadSnapshot};
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
            task_events: super::super::task_events::TaskEventsSnapshot::default(),
            io: IoMonitorSnapshot::default(),
            cache: CacheMonitorSnapshot::default(),
            threads: ThreadSnapshot { threads: Vec::new() },
            captured_at: Some(SystemTime::UNIX_EPOCH),
            captured_instant: Some(Instant::now()),
            filter_index: None,
            filter_summary: super::super::filter_index::FilterSummary::default(),
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
            Screen::Browse | Screen::Offline { .. } => None,
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

        let popup = RecordingConfigurationPopup::new(configuration);
        assert_eq!(
            popup
                .fields()
                .into_iter()
                .map(|field| (field.label(), field.value(popup)))
                .collect::<Vec<_>>(),
            vec![
                ("Allocations", "custom".into()),
                ("  Backtraces", "on".into()),
                ("  Sampling", "1/8 (12.5%)".into()),
                ("Events", "custom".into()),
                ("  Backtraces", "on".into()),
                ("  Sampling", "1/20 (5%)".into()),
                ("Arc", "custom".into()),
                ("  Backtraces", "on".into()),
                ("  Sampling", "1/100 (1%)".into()),
                ("Runtime tasks", "on".into()),
                ("I/O", "custom".into()),
                ("  Backtraces", "on".into()),
                ("  Sampling", "1/4 (25%)".into()),
                ("Cache", "custom".into()),
                ("  Backtraces", "on".into()),
                ("  Sampling", "1/10 (10%)".into()),
                ("Event buffer capacity", "1024 events / thread".into()),
            ]
        );
    }

    #[test]
    fn recording_modes_expand_custom_fields_and_normalize_on_apply() {
        let mut popup = RecordingConfigurationPopup::new(RecordingConfiguration::default());
        RecordingConfigurationField::AllocationRecording.adjust(&mut popup, 1);
        let on_fields = popup.fields();
        RecordingConfigurationField::AllocationRecording.adjust(&mut popup, 1);
        let custom_fields = popup.fields();
        RecordingConfigurationField::AllocationBacktraces.adjust(&mut popup, 1);
        RecordingConfigurationField::AllocationSampling.adjust(&mut popup, 1);
        let configuration = popup.configuration();
        assert_eq!(
            (on_fields, custom_fields, configuration.allocations,),
            (
                vec![
                    RecordingConfigurationField::AllocationRecording,
                    RecordingConfigurationField::GeneralRecording,
                    RecordingConfigurationField::ArcDereferenceRecording,
                    RecordingConfigurationField::RuntimeTaskRecording,
                    RecordingConfigurationField::IoRecording,
                    RecordingConfigurationField::CacheRecording,
                    RecordingConfigurationField::EventBufferCapacity,
                ],
                vec![
                    RecordingConfigurationField::AllocationRecording,
                    RecordingConfigurationField::AllocationBacktraces,
                    RecordingConfigurationField::AllocationSampling,
                    RecordingConfigurationField::GeneralRecording,
                    RecordingConfigurationField::ArcDereferenceRecording,
                    RecordingConfigurationField::RuntimeTaskRecording,
                    RecordingConfigurationField::IoRecording,
                    RecordingConfigurationField::CacheRecording,
                    RecordingConfigurationField::EventBufferCapacity,
                ],
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 2,
                },
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
        runtime.focus = RuntimeFocus::Activity;
        runtime.worker_selected = 2;
        runtime.task_selected = 3;
        runtime.activity_scroll = 4;
        runtime.reset();
        let mut io = IoViewState {
            focus: IoFocus::Operations,
            resource_selected: 2,
            operation_selected: 3,
        };
        io.reset();
        let mut cache = CacheViewState {
            focus: CacheFocus::Operations,
            tier_selected: 2,
            operation_selected: 3,
        };
        cache.reset();

        assert_eq!(
            (allocation, primitive, heap, thread, runtime, io, cache),
            (
                AllocationViewState::new(),
                PrimitiveViewState::new(),
                HeapViewState::new(),
                ThreadViewState::new(),
                RuntimeViewState::new(),
                IoViewState::new(),
                CacheViewState::new(),
            )
        );
    }

    #[test]
    fn event_buffer_capacity_is_not_associated_with_a_recorder() {
        assert_eq!(RecordingConfigurationField::EventBufferCapacity.recorder(), None);
    }

    macro_rules! recording_configuration_regressions {
        ($cycle:ident, $backtraces:ident, $policy:ident, $mode:ident, $trace_field:ident, $sampling:expr) => {
            #[test]
            fn $cycle() {
                let mut configuration = RecordingConfiguration::default();
                configuration.$policy.enabled = true;
                configuration.$policy.capture_backtraces = $sampling != 1;
                configuration.$policy.sampling_one_in = $sampling;
                let mut app = connected_app(MonitorTab::Info);
                let mut popup = RecordingConfigurationPopup::new(configuration);
                popup.selected = popup
                    .fields()
                    .iter()
                    .position(|field| *field == RecordingConfigurationField::$mode)
                    .unwrap();
                app.recording_configuration_popup = Some(popup);

                app.handle_key(KeyCode::Left);
                let on = app.recording_configuration_popup.unwrap();
                app.handle_key(KeyCode::Left);
                let off = app.recording_configuration_popup.unwrap();
                app.handle_key(KeyCode::Left);
                let lower_bound = app.recording_configuration_popup.unwrap();
                app.handle_key(KeyCode::Char(' '));
                app.handle_key(KeyCode::Right);
                let restored = app.recording_configuration_popup.unwrap();
                app.handle_key(KeyCode::Right);
                let upper_bound = app.recording_configuration_popup.unwrap();

                let mut enabled = configuration;
                enabled.$policy.capture_backtraces = true;
                enabled.$policy.sampling_one_in = 1;
                let mut disabled = configuration;
                disabled.$policy.enabled = false;
                assert_eq!(
                    (
                        on.configuration(),
                        off.configuration(),
                        lower_bound,
                        restored,
                        upper_bound,
                        on.field(),
                        off.field(),
                    ),
                    (
                        enabled,
                        disabled,
                        off,
                        popup,
                        popup,
                        RecordingConfigurationField::$mode,
                        RecordingConfigurationField::$mode,
                    )
                );
            }

            #[test]
            fn $backtraces() {
                let mut configuration = RecordingConfiguration::default();
                configuration.$policy.enabled = true;
                configuration.$policy.capture_backtraces = true;
                let mut app = connected_app(MonitorTab::Info);
                let mut popup = RecordingConfigurationPopup::new(configuration);
                RecordingConfigurationField::$mode.adjust(&mut popup, 1);
                popup.selected = popup
                    .fields()
                    .iter()
                    .position(|field| *field == RecordingConfigurationField::$trace_field)
                    .unwrap();
                app.recording_configuration_popup = Some(popup);

                app.handle_key(KeyCode::Right);
                let toggled = app.recording_configuration_popup.unwrap();
                app.handle_key(KeyCode::Left);
                let restored = app.recording_configuration_popup.unwrap();

                let mut expected = configuration;
                expected.$policy.capture_backtraces = false;
                assert_eq!(
                    (toggled.configuration(), toggled.fields(), restored),
                    (expected, popup.fields(), popup)
                );
            }
        };
    }

    recording_configuration_regressions!(
        allocation_modes_preserve_custom_draft,
        allocation_backtraces_toggle_without_collapsing_custom_fields,
        allocations,
        AllocationRecording,
        AllocationBacktraces,
        8
    );
    recording_configuration_regressions!(
        general_modes_preserve_custom_draft,
        general_backtraces_toggle_without_collapsing_custom_fields,
        general_events,
        GeneralRecording,
        GeneralBacktraces,
        8
    );
    recording_configuration_regressions!(
        arc_modes_preserve_custom_draft,
        arc_backtraces_toggle_without_collapsing_custom_fields,
        arc_dereferences,
        ArcDereferenceRecording,
        ArcDereferenceBacktraces,
        8
    );
    recording_configuration_regressions!(
        runtime_modes_preserve_custom_draft,
        runtime_backtraces_toggle_without_collapsing_custom_fields,
        runtime_tasks,
        RuntimeTaskRecording,
        RuntimeTaskBacktraces,
        1
    );
    recording_configuration_regressions!(
        io_modes_preserve_custom_draft,
        io_backtraces_toggle_without_collapsing_custom_fields,
        io,
        IoRecording,
        IoBacktraces,
        8
    );
    recording_configuration_regressions!(
        cache_modes_preserve_custom_draft,
        cache_backtraces_toggle_without_collapsing_custom_fields,
        cache,
        CacheRecording,
        CacheBacktraces,
        8
    );

    macro_rules! recording_sampling_regression {
        ($name:ident, $policy:ident, $field:ident) => {
            #[test]
            fn $name() {
                let mut configuration = RecordingConfiguration::default();
                configuration.$policy.enabled = true;
                configuration.$policy.sampling_one_in = 8;
                let mut app = connected_app(MonitorTab::Info);
                let mut popup = RecordingConfigurationPopup::new(configuration);
                popup.selected = popup
                    .fields()
                    .iter()
                    .position(|field| *field == RecordingConfigurationField::$field)
                    .unwrap();
                app.recording_configuration_popup = Some(popup);

                app.handle_key(KeyCode::Right);
                let increased = app.recording_configuration_popup.unwrap().configuration();
                app.handle_key(KeyCode::Left);
                let restored = app.recording_configuration_popup.unwrap();
                let mut expected = configuration;
                expected.$policy.sampling_one_in = 16;
                assert_eq!((increased, restored), (expected, popup));
            }
        };
    }

    recording_sampling_regression!(allocation_sampling_edits_only_allocations, allocations, AllocationSampling);
    recording_sampling_regression!(general_sampling_edits_only_events, general_events, GeneralSampling);
    recording_sampling_regression!(arc_sampling_edits_only_arc, arc_dereferences, ArcDereferenceSampling);
    recording_sampling_regression!(io_sampling_edits_only_io, io, IoSampling);
    recording_sampling_regression!(cache_sampling_edits_only_cache, cache, CacheSampling);

    #[test]
    fn configuration_keyboard_navigation_clamps_and_edits_capacity() {
        let mut app = connected_app(MonitorTab::Info);
        app.handle_key(KeyCode::Char('c'));
        let original = app.recording_configuration_popup.unwrap();
        app.handle_key(KeyCode::Up);
        let at_start = app.recording_configuration_popup.unwrap();
        for _ in 0..original.fields().len() {
            app.handle_key(KeyCode::Down);
        }
        app.handle_key(KeyCode::Right);
        let increased = app.recording_configuration_popup.unwrap();
        app.handle_key(KeyCode::Left);
        let restored = app.recording_configuration_popup.unwrap();
        app.handle_key(KeyCode::Up);
        let previous = app.recording_configuration_popup.unwrap().field();

        let mut expected = original.draft;
        expected.event_capacity_per_thread = 131_072;
        assert_eq!(
            (
                at_start,
                increased.configuration(),
                restored.configuration(),
                restored.field(),
                previous
            ),
            (
                original,
                expected,
                original.draft,
                RecordingConfigurationField::EventBufferCapacity,
                RecordingConfigurationField::CacheRecording,
            )
        );
    }

    #[test]
    fn tabs_cover_every_variant() {
        assert_eq!(
            [
                MonitorTab::Info,
                MonitorTab::Heaps,
                MonitorTab::Allocations,
                MonitorTab::Primitives,
                MonitorTab::Threads,
                MonitorTab::Runtime,
                MonitorTab::Io,
                MonitorTab::Cache,
            ]
            .map(|tab| (tab.index(), tab.next(), tab.previous())),
            [
                (0, MonitorTab::Heaps, MonitorTab::Cache),
                (1, MonitorTab::Allocations, MonitorTab::Info),
                (2, MonitorTab::Primitives, MonitorTab::Heaps),
                (3, MonitorTab::Threads, MonitorTab::Allocations),
                (4, MonitorTab::Runtime, MonitorTab::Primitives),
                (5, MonitorTab::Io, MonitorTab::Threads),
                (6, MonitorTab::Cache, MonitorTab::Runtime),
                (7, MonitorTab::Info, MonitorTab::Io),
            ]
        );
    }

    #[test]
    fn next_tab_wraps_to_info() {
        assert_eq!(MonitorTab::Cache.next(), MonitorTab::Info);
    }

    #[test]
    fn previous_tab_wraps_to_cache() {
        assert_eq!(MonitorTab::Info.previous(), MonitorTab::Cache);
    }

    #[test]
    fn recording_mode_changes_preserve_custom_values_until_apply() {
        let mut configuration = RecordingConfiguration::default();
        configuration.allocations.enabled = true;
        configuration.allocations.capture_backtraces = true;
        configuration.allocations.sampling_one_in = 8;
        let mut popup = RecordingConfigurationPopup::new(configuration);
        RecordingConfigurationField::AllocationRecording.adjust(&mut popup, -1);

        assert_eq!(
            (
                popup.mode(RecorderKind::Allocations),
                popup.draft.allocations.capture_backtraces,
                popup.configuration().allocations,
            ),
            (
                RecordingMode::On,
                true,
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 1,
                }
            )
        );
    }

    #[test]
    fn backtrace_toggle_preserves_recording() {
        let mut popup = RecordingConfigurationPopup::new(RecordingConfiguration::default());
        RecordingConfigurationField::GeneralRecording.adjust(&mut popup, 1);
        RecordingConfigurationField::GeneralRecording.adjust(&mut popup, 1);
        RecordingConfigurationField::GeneralBacktraces.adjust(&mut popup, 1);

        assert_eq!(
            (
                popup.configuration().general_events.enabled,
                popup.configuration().general_events.capture_backtraces
            ),
            (true, true)
        );
    }

    #[test]
    fn configuration_choices_cover_protocol_bounds() {
        assert_eq!(
            (
                EVENT_BUFFER_CAPACITIES.first().copied(),
                EVENT_BUFFER_CAPACITIES.last().copied(),
                adjusted_value(1, &EVENT_BUFFER_CAPACITIES, -1),
                adjusted_value(1_000, &EVENT_BUFFER_CAPACITIES, 0),
                adjusted_value(u32::MAX, &EVENT_BUFFER_CAPACITIES, 1),
                EVENT_SAMPLING_RATES.first().copied(),
                EVENT_SAMPLING_RATES.last().copied(),
                adjusted_value(20, &EVENT_SAMPLING_RATES, -1),
                adjusted_value(1_000, &EVENT_SAMPLING_RATES, 0),
                advance_selection(0, 0),
                advance_selection(0, 3),
                advance_selection(2, 3),
            ),
            (
                Some(64),
                Some(1_048_576),
                64,
                1_024,
                1_048_576,
                Some(1),
                Some(65_536),
                16,
                1_024,
                0,
                1,
                2,
            )
        );
    }

    #[test]
    fn configuration_popup_uses_arrow_keys_and_escape() {
        let mut app = App::new();
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        app.handle_key(KeyCode::Down);
        let moved = app.recording_configuration_popup;
        app.handle_key(KeyCode::Esc);

        assert_eq!(moved.map(|popup| popup.selected), Some(1));
        assert_eq!(app.recording_configuration_popup, None);
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
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        app.handle_recording_configuration_key(KeyCode::Char('x'));

        assert_eq!((app.next_refresh(), app.status), (refresh, String::new()));
    }

    #[test]
    fn configuration_popup_handles_modes_custom_settings_and_disconnected_apply() {
        let mut app = App::new();
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        app.handle_key(KeyCode::Right);
        let after_right = app.recording_configuration_popup.unwrap();
        app.handle_key(KeyCode::Right);
        let after_custom = app.recording_configuration_popup.unwrap();
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Right);
        let backtraces = app.recording_configuration_popup.unwrap().draft.allocations.capture_backtraces;
        app.handle_key(KeyCode::Enter);

        assert_eq!(
            (
                after_right.mode(RecorderKind::Allocations),
                after_custom.mode(RecorderKind::Allocations),
                backtraces,
                app.recording_configuration_popup,
            ),
            (RecordingMode::On, RecordingMode::Custom, true, None)
        );
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

        assert_eq!(app.recording_configuration_popup, Some(RecordingConfigurationPopup::new(recording)));
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
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        app.handle_key(KeyCode::Esc);

        assert_eq!(
            (connected_fields(&app.screen).unwrap().1, app.recording_configuration_popup),
            (RecordingConfiguration::default(), None)
        );
    }

    #[test]
    fn capture_mode_has_two_actions_and_defaults_to_continue() {
        assert_eq!(
            (CaptureMode::default(), CaptureMode::Continue.next(), CaptureMode::Stop.next()),
            (CaptureMode::Continue, CaptureMode::Stop, CaptureMode::Continue)
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn browser_and_connected_keys_update_screen_state() {
        let mut app = App::new();
        app.instances = vec![
            Instance {
                descriptor: descriptor(1),
                recording: RecordingConfiguration::default(),
                server_version: None,
            },
            Instance {
                descriptor: descriptor(2),
                recording: RecordingConfiguration::default(),
                server_version: None,
            },
        ];
        assert!(app.handle_key(KeyCode::Char('q')));
        assert!(app.handle_key(KeyCode::Char('Q')));
        assert!(!app.handle_key(KeyCode::Char('x')));
        app.handle_key(KeyCode::Down);
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Down);
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Up);
        assert_eq!(app.selected, 0);
        app.handle_key(KeyCode::Enter);
        assert_eq!(connected_fields(&app.screen).unwrap().0, descriptor(1).instance_id);

        for (key, expected) in [
            (KeyCode::Char('2'), MonitorTab::Heaps),
            (KeyCode::Char('3'), MonitorTab::Allocations),
            (KeyCode::Char('4'), MonitorTab::Primitives),
            (KeyCode::Char('5'), MonitorTab::Threads),
            (KeyCode::Char('6'), MonitorTab::Runtime),
            (KeyCode::Char('7'), MonitorTab::Io),
            (KeyCode::Char('8'), MonitorTab::Cache),
            (KeyCode::Char('1'), MonitorTab::Info),
            (KeyCode::Left, MonitorTab::Cache),
            (KeyCode::Right, MonitorTab::Info),
            (KeyCode::Char('h'), MonitorTab::Cache),
            (KeyCode::Char('l'), MonitorTab::Info),
            (KeyCode::BackTab, MonitorTab::Cache),
            (KeyCode::Tab, MonitorTab::Info),
        ] {
            app.handle_key(key);
            assert_eq!(connected_fields(&app.screen).unwrap().2, expected);
        }
        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.capture_mode, CaptureMode::Stop);
        app.handle_key(KeyCode::Char('x'));
        app.handle_key(KeyCode::Esc);
        assert!(matches!(app.screen, Screen::Browse));

        let mut browser = App::new();
        browser.handle_key(KeyCode::Char('r'));
        assert!(browser.discovery_receiver.is_some());
    }

    #[test]
    fn connected_tab_dispatch_updates_only_the_active_view() {
        let cases = [
            (MonitorTab::Heaps, KeyCode::Char(']')),
            (MonitorTab::Allocations, KeyCode::Char(']')),
            (MonitorTab::Primitives, KeyCode::Char(']')),
            (MonitorTab::Threads, KeyCode::Char('f')),
            (MonitorTab::Runtime, KeyCode::Char(']')),
            (MonitorTab::Io, KeyCode::Enter),
            (MonitorTab::Cache, KeyCode::Enter),
        ];

        let actual = cases.map(|(tab, key)| {
            let mut app = connected_app(tab);
            app.handle_key(key);
            (
                app.heap_view.tier,
                app.allocation_view.sort,
                app.primitive_view.sort,
                app.thread_view.stack_filter,
                app.runtime_view.task_sort,
                app.io_view.focus,
                app.cache_view.focus,
                connected_fields(&app.screen).unwrap().2,
            )
        });

        assert_eq!(
            actual,
            [
                (
                    MemoryTier::Medium,
                    AllocationSort::Allocations,
                    PrimitiveSort::Events,
                    AllocationStackFilter::Application,
                    RuntimeTaskSort::Polls,
                    IoFocus::Resources,
                    CacheFocus::Tiers,
                    MonitorTab::Heaps,
                ),
                (
                    MemoryTier::Small,
                    AllocationSort::AllocatedBytes,
                    PrimitiveSort::Events,
                    AllocationStackFilter::Application,
                    RuntimeTaskSort::Polls,
                    IoFocus::Resources,
                    CacheFocus::Tiers,
                    MonitorTab::Allocations,
                ),
                (
                    MemoryTier::Small,
                    AllocationSort::Allocations,
                    PrimitiveSort::Objects,
                    AllocationStackFilter::Application,
                    RuntimeTaskSort::Polls,
                    IoFocus::Resources,
                    CacheFocus::Tiers,
                    MonitorTab::Primitives,
                ),
                (
                    MemoryTier::Small,
                    AllocationSort::Allocations,
                    PrimitiveSort::Events,
                    AllocationStackFilter::All,
                    RuntimeTaskSort::Polls,
                    IoFocus::Resources,
                    CacheFocus::Tiers,
                    MonitorTab::Threads,
                ),
                (
                    MemoryTier::Small,
                    AllocationSort::Allocations,
                    PrimitiveSort::Events,
                    AllocationStackFilter::Application,
                    RuntimeTaskSort::Executing,
                    IoFocus::Resources,
                    CacheFocus::Tiers,
                    MonitorTab::Runtime,
                ),
                (
                    MemoryTier::Small,
                    AllocationSort::Allocations,
                    PrimitiveSort::Events,
                    AllocationStackFilter::Application,
                    RuntimeTaskSort::Polls,
                    IoFocus::Operations,
                    CacheFocus::Tiers,
                    MonitorTab::Io,
                ),
                (
                    MemoryTier::Small,
                    AllocationSort::Allocations,
                    PrimitiveSort::Events,
                    AllocationStackFilter::Application,
                    RuntimeTaskSort::Polls,
                    IoFocus::Resources,
                    CacheFocus::Operations,
                    MonitorTab::Cache,
                ),
            ]
        );
    }

    #[test]
    fn recording_popup_is_blocked_during_capture_and_recording_updates() {
        let mut app = connected_app(MonitorTab::Info);
        let (_capture_sender, capture_receiver) = unbounded();
        app.capture_receiver = Some(capture_receiver);
        app.handle_key(KeyCode::Char('c'));
        let while_capturing = app.recording_configuration_popup;
        app.recording_configuration_popup = None;
        app.capture_receiver = None;
        let (_recording_sender, recording_receiver) = unbounded();
        app.recording_receiver = Some(recording_receiver);
        app.handle_key(KeyCode::Char('c'));
        let while_recording = app.recording_configuration_popup;
        app.recording_receiver = None;
        app.handle_key(KeyCode::Char('c'));

        assert_eq!(
            (
                while_capturing.is_some(),
                while_recording,
                app.recording_configuration_popup.is_some()
            ),
            (false, None, true)
        );
    }

    #[test]
    fn refresh_schedules_the_next_worker_for_each_screen() {
        let mut browse = App::new();
        let before = browse.next_refresh();
        browse.refresh_with(|| Ok(Vec::new()), |_descriptor| Ok(recorder_statistics_with_total(0).into()));
        let browse_state = (browse.next_refresh() > before, browse.discovery_receiver.is_some());

        let mut connected = connected_app(MonitorTab::Info);
        connected.refresh_with(|| Ok(Vec::new()), |_descriptor| Ok(recorder_statistics_with_total(0).into()));
        let connected_state = (connected.statistics_receiver.is_some(), connected.discovery_receiver.is_none());

        assert_eq!((browse_state, connected_state), ((true, true), (true, true)));
        assert_eq!(
            [
                workers_are_idle(false, false, false),
                workers_are_idle(true, false, false),
                workers_are_idle(false, true, false),
                workers_are_idle(false, false, true),
                workers_are_idle(true, true, false),
                workers_are_idle(true, false, true),
                workers_are_idle(false, true, true),
                workers_are_idle(true, true, true),
            ],
            [true, false, false, false, false, false, false, false]
        );
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
        assert!(!view.descending);
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

        let tiers = [
            MemoryTierData {
                kind: MemoryTier::Small,
                current_allocations: 0,
                current_bytes: 0,
                buckets: Vec::new(),
            },
            MemoryTierData {
                kind: MemoryTier::Medium,
                current_allocations: 0,
                current_bytes: 0,
                buckets: Vec::new(),
            },
        ];
        assert_eq!(
            tier_with_kind(&tiers, MemoryTier::Medium).map(|tier| tier.kind),
            Some(MemoryTier::Medium)
        );
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

        assert_eq!(
            (
                app.activity_samples.len(),
                app.activity_samples.front().map(|sample| sample.total_events),
                app.activity_samples.back().map(|sample| sample.total_events),
            ),
            (MAX_ACTIVITY_SAMPLES, Some(2), Some(MAX_ACTIVITY_SAMPLES as u64 + 1))
        );
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
        let mut app = connected_app(MonitorTab::Info);
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);

        app.finish_snapshot_capture(Err("snapshot decode failed".into()));

        assert_eq!(app.snapshot_error.as_deref(), Some("snapshot decode failed"));
    }

    #[test]
    fn clear_preserves_the_captured_view_and_resets_activity_baselines() {
        let mut app = connected_app(MonitorTab::Info);
        let snapshot_before = match &app.screen {
            Screen::Connected { snapshot, .. } => std::ptr::from_ref(snapshot.as_ref().unwrap().as_ref()),
            _ => unreachable!(),
        };
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);
        app.record_activity(RecorderStatistics {
            total_events: 100,
            ..Default::default()
        });
        app.record_activity(RecorderStatistics {
            total_events: 110,
            ..Default::default()
        });
        let (sender, receiver) = unbounded();
        app.capture_receiver = Some(receiver);
        sender
            .send(CaptureMessage::Recording(Ok(RecordingConfiguration::default())))
            .unwrap();
        sender.send(CaptureMessage::Cleared(Ok(()))).unwrap();
        app.poll_snapshot_capture();
        let snapshot_after = match &app.screen {
            Screen::Connected { snapshot, .. } => std::ptr::from_ref(snapshot.as_ref().unwrap().as_ref()),
            _ => unreachable!(),
        };
        assert_eq!(snapshot_before, snapshot_after);
        assert!(app.status.contains("Server buffers cleared"));
        assert!(app.activity_samples.is_empty());
        assert!(app.recorder_statistics.is_none());
        assert!(app.activity_observed_at.is_none());
        assert!(app.capture_receiver.is_none());
        assert!(app.capture_started_at.is_none());
    }

    #[test]
    fn stale_capture_and_clear_completions_cannot_mutate_a_new_connection() {
        let mut app = connected_app(MonitorTab::Info);
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);
        app.connection_generation += 1;
        app.status = "new connection".into();
        let (sender, receiver) = unbounded();
        app.capture_receiver = Some(receiver);
        sender.send(CaptureMessage::Recording(Err("old connection failed".into()))).unwrap();
        sender.send(CaptureMessage::Cleared(Err("old clear failed".into()))).unwrap();
        app.poll_snapshot_capture();
        app.finish_snapshot_capture(Err("old capture failed".into()));
        assert_eq!(
            (app.status.as_str(), app.recording_unknown, app.snapshot_error),
            ("new connection", false, None)
        );
    }

    #[test]
    fn failed_reconciliation_marks_state_unknown_without_claiming_deallocation() {
        let mut app = connected_app(MonitorTab::Info);
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);
        app.reconcile_recording(Err("connection lost".into()));
        app.finish_snapshot_capture(Err("stop response lost".into()));
        app.handle_key(KeyCode::Char('c'));
        assert!(app.recording_unknown);
        assert_eq!(app.status, "stop response lost");
        assert!(app.recording_configuration_popup.is_none());
    }

    #[test]
    fn lifecycle_keys_are_blocked_by_configuration_and_modal_drafts() {
        let mut app = connected_app(MonitorTab::Info);
        let (_sender, receiver) = unbounded();
        app.recording_receiver = Some(receiver);
        for key in ['s', 'C'] {
            app.handle_key(KeyCode::Char(key));
        }
        assert!(app.capture_receiver.is_none());
        app.recording_receiver = None;
        app.handle_key(KeyCode::Char('c'));
        let draft = app.recording_configuration_popup;
        for key in ['s', 'C', 'd'] {
            app.handle_key(KeyCode::Char(key));
        }
        assert_eq!(app.recording_configuration_popup, draft);
        assert_eq!(app.capture_mode, CaptureMode::Continue);
        assert!(app.capture_receiver.is_none());
        app.recording_configuration_popup = None;
        app.handle_key(KeyCode::F(1));
        for key in ['s', 'C', 'd'] {
            app.handle_key(KeyCode::Char(key));
        }
        assert!(app.capture_receiver.is_none());
        assert_eq!(app.capture_mode, CaptureMode::Continue);
    }

    #[test]
    fn stop_worker_reconciles_server_policy_after_a_capture_error() {
        use std::net::TcpListener;

        use seismograph_protocol::message::{Request, Response};
        use seismograph_protocol::{read_request, write_response};
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut descriptor = descriptor(1);
        descriptor.port = listener.local_addr().unwrap().port();
        let id = descriptor.instance_id;
        let worker = thread::spawn(move || {
            for stop in [true, false] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                assert!(matches!(read_request(&mut stream).unwrap(), (1, Request::Hello { .. })));
                write_response(
                    &mut stream,
                    1,
                    &Response::Hello {
                        instance_id: id,
                        recording: RecordingConfiguration::default(),
                    },
                )
                .unwrap();
                let request = read_request(&mut stream).unwrap();
                let response = if stop {
                    assert_eq!(request, (2, Request::CaptureSnapshotAndStop));
                    Response::Error("source failed after recorder cleanup".into())
                } else {
                    assert_eq!(request, (2, Request::ReadCacheRecording));
                    Response::CacheRecording(seismograph_protocol::message::RecordingPolicy::default())
                };
                write_response(&mut stream, 2, &response).unwrap();
            }
        });
        let (sender, receiver) = unbounded();
        let error = capture_connected_snapshot(&descriptor, CaptureMode::Stop, &sender).err().unwrap();
        worker.join().unwrap();
        assert!(error.contains("source failed after recorder cleanup"));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            CaptureMessage::Progress(CaptureStep::Capture)
        ));
        assert!(matches!(receiver.try_recv().unwrap(), CaptureMessage::Recording(Ok(configuration))
            if configuration == RecordingConfiguration::default()));
        receiver.try_recv().err().unwrap();
    }

    #[test]
    fn successful_snapshot_capture_resets_views_and_installs_snapshot() {
        let mut app = connected_app(MonitorTab::Info);
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);
        app.heap_view.focus = HeapFocus::Hotspots;
        app.allocation_view.selected = 3;
        app.thread_view.focus = ThreadFocus::Objects;
        app.runtime_view.focus = RuntimeFocus::Activity;
        app.io_view.focus = IoFocus::Operations;
        app.cache_view.focus = CacheFocus::Operations;
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
                app.io_view,
                app.cache_view,
                app.status.as_str(),
            ),
            (
                true,
                HeapViewState::new(),
                AllocationViewState::new(),
                ThreadViewState::new(),
                RuntimeViewState::new(),
                IoViewState::new(),
                CacheViewState::new(),
                "saved",
            )
        );
    }

    #[test]
    fn snapshot_capture_messages_cover_progress_completion_and_closed_worker() {
        let mut app = connected_app(MonitorTab::Info);
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);
        let (sender, receiver) = unbounded();
        app.capture_receiver = Some(receiver);
        sender.send(CaptureMessage::Progress(CaptureStep::Decode)).unwrap();
        sender.send(CaptureMessage::Complete(Err("decode failed".into()))).unwrap();
        app.poll_snapshot_capture();
        assert_eq!((app.capture_step, app.snapshot_error.as_deref()), (None, Some("decode failed")));

        let (sender, receiver) = unbounded::<CaptureMessage>();
        drop(sender);
        app.capture_receiver = Some(receiver);
        app.capture_instance_id = connected_fields(&app.screen).map(|fields| fields.0);
        app.poll_snapshot_capture();
        assert_eq!(app.snapshot_error.as_deref(), Some("Snapshot capture worker stopped unexpectedly"));
    }

    #[test]
    fn worker_receivers_distinguish_empty_closed_and_available_channels() {
        let (capture_sender, capture_receiver) = unbounded();
        assert!(matches!(receive_capture_message(Some(&capture_receiver)), Ok(None)));
        capture_sender.send(CaptureMessage::Progress(CaptureStep::Save)).unwrap();
        assert!(matches!(
            receive_capture_message(Some(&capture_receiver)),
            Ok(Some(CaptureMessage::Progress(CaptureStep::Save)))
        ));
        drop(capture_sender);
        assert!(matches!(
            receive_capture_message(Some(&capture_receiver)),
            Err(error) if error == "Snapshot capture worker stopped unexpectedly"
        ));
        assert!(matches!(receive_capture_message(None), Ok(None)));

        let (worker_sender, worker_receiver) = unbounded();
        assert!(receive_worker_result::<u64>(Some(&worker_receiver), "test").is_none());
        worker_sender.send(Ok(42)).unwrap();
        assert_eq!(receive_worker_result(Some(&worker_receiver), "test"), Some(Ok(42)));
        drop(worker_sender);
        assert_eq!(
            receive_worker_result::<u64>(Some(&worker_receiver), "test"),
            Some(Err("test worker stopped unexpectedly".into()))
        );
        assert_eq!(receive_worker_result::<u64>(None, "test"), None);
    }

    #[test]
    fn discovery_messages_preserve_selection_and_handle_errors() {
        let mut app = App::new();
        app.instances = vec![Instance {
            descriptor: descriptor(2),
            recording: RecordingConfiguration::default(),
            server_version: None,
        }];
        let (sender, receiver) = unbounded();
        sender
            .send(Ok(vec![
                Instance {
                    descriptor: descriptor(1),
                    recording: RecordingConfiguration::default(),
                    server_version: None,
                },
                Instance {
                    descriptor: descriptor(2),
                    recording: RecordingConfiguration::default(),
                    server_version: None,
                },
            ]))
            .unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();
        assert_eq!((app.selected, app.status.as_str()), (1, "2 application(s) available"));

        let (sender, receiver) = unbounded();
        sender.send(Ok(Vec::new())).unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();
        assert_eq!(app.status, "No reachable Seismograph monitors found");

        let (sender, receiver) = unbounded();
        sender.send(Err("discovery failed".into())).unwrap();
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
        sender.send(Ok(Vec::new())).unwrap();
        app.discovery_receiver = Some(receiver);
        app.poll_discovery();

        assert!(matches!(app.screen, Screen::Connected { .. }));
    }

    #[test]
    fn detailed_statistics_refresh_without_capture_and_info_navigation_keeps_thread_identity() {
        use seismograph_protocol::message::{EventClassCounts, RecorderActivity, ThreadRecorderStatistics};
        let mut app = connected_app(MonitorTab::Info);
        if let Screen::Connected { snapshot, .. } = &mut app.screen {
            *snapshot = None;
        }
        let activity = |session_id, ids: &[u64]| RecorderActivity {
            session_id,
            statistics: recorder_statistics_with_total(100),
            class_events: EventClassCounts {
                general_events: 100,
                ..EventClassCounts::default()
            },
            threads: ids
                .iter()
                .map(|&thread_id| ThreadRecorderStatistics {
                    thread_id,
                    name: format!("worker-{thread_id}"),
                    total_events: 100,
                    retained_events: 64,
                    lost_events: 36,
                    event_capacity: 64,
                    ..ThreadRecorderStatistics::default()
                })
                .collect(),
        };
        let (sender, receiver) = unbounded();
        sender.send(Ok(RecorderUpdate::Detailed(activity(1, &[2, 4, 8])))).unwrap();
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert!(matches!(app.screen, Screen::Connected { snapshot: None, .. }));
        assert_eq!(app.live_activity.availability, super::super::live_activity::Availability::Supported);
        app.handle_key(KeyCode::Down);
        assert_eq!(app.info_thread_selected, 1);
        app.record_recorder_update(RecorderUpdate::Detailed(activity(1, &[1, 2, 4, 8])));
        assert_eq!(app.live_activity.threads[app.info_thread_selected].statistics.thread_id, 4);
        app.handle_key(KeyCode::End);
        assert_eq!(app.info_thread_selected, 3);
        app.handle_key(KeyCode::Home);
        app.handle_key(KeyCode::PageDown);
        assert_eq!(app.info_thread_selected, 3);
        app.handle_key(KeyCode::PageUp);
        assert_eq!(app.info_thread_selected, 0);
        app.handle_key(KeyCode::Tab);
        assert!(matches!(
            app.screen,
            Screen::Connected {
                tab: MonitorTab::Heaps,
                ..
            }
        ));
        app.record_recorder_update(RecorderUpdate::Detailed(activity(2, &[4])));
        assert!(app.live_activity.samples.is_empty() && app.activity_samples.is_empty());
    }

    #[test]
    fn statistics_messages_record_activity_and_handle_errors() {
        let mut app = connected_app(MonitorTab::Info);
        let (sender, receiver) = unbounded();
        sender.send(Ok(recorder_statistics_with_total(10).into())).unwrap();
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert_eq!(app.recorder_statistics.as_ref().map(|value| value.total_events), Some(10));

        let (sender, receiver) = unbounded();
        sender.send(Err("statistics failed".into())).unwrap();
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert_eq!(app.status, "statistics failed");
        assert!(app.live_activity.stale);

        let (sender, receiver) = unbounded::<Result<RecorderUpdate, String>>();
        drop(sender);
        app.statistics_receiver = Some(receiver);
        app.poll_recorder_statistics();
        assert_eq!(app.status, "Recorder statistics worker stopped unexpectedly");
    }

    #[test]
    fn recording_configuration_messages_update_the_connected_instance() {
        let mut app = connected_app(MonitorTab::Info);
        app.poll_recording_configuration();
        let mut configuration = RecordingConfiguration::default();
        configuration.io.enabled = true;
        let (sender, receiver) = unbounded();
        sender
            .send(Ok(RecordingUpdate {
                descriptor: descriptor(1),
                configuration,
            }))
            .unwrap();
        app.recording_receiver = Some(receiver);

        app.poll_recording_configuration();

        assert_eq!(
            connected_fields(&app.screen).map(|(_, recording, _, _)| recording.io.enabled),
            Some(true)
        );

        let (sender, receiver) = unbounded();
        sender.send(Err("configuration failed".into())).unwrap();
        app.recording_receiver = Some(receiver);
        app.poll_recording_configuration();
        assert_eq!(app.status, "configuration failed");
    }

    #[test]
    fn recording_configuration_apply_starts_a_worker() {
        let mut app = connected_app(MonitorTab::Info);
        let mut configuration = RecordingConfiguration::default();
        configuration.io.enabled = true;

        app.apply_recording_configuration_with(configuration, |_descriptor, configuration| Ok(configuration));
        let result = app.recording_receiver.take().unwrap().recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(
            (app.recording_configuration_popup, result.is_ok(), app.status.as_str(),),
            (None, true, "Applying recording configuration...")
        );
    }

    #[test]
    fn live_recording_refresh_preserves_popup_edits() {
        let mut app = connected_app(MonitorTab::Info);
        let mut draft = RecordingConfigurationPopup::new(RecordingConfiguration::default());
        RecordingConfigurationField::AllocationRecording.adjust(&mut draft, 1);
        app.recording_configuration_popup = Some(draft);
        let mut statistics = recorder_statistics_with_total(10);
        statistics.recording.cache.enabled = true;
        statistics.recording.io.capture_backtraces = true;
        let (sender, receiver) = unbounded();
        sender.send(Ok(statistics.into())).unwrap();
        app.statistics_receiver = Some(receiver);

        app.poll_recorder_statistics();

        assert_eq!(
            (
                connected_fields(&app.screen).unwrap().1,
                app.recorder_statistics,
                app.recording_configuration_popup,
            ),
            (statistics.recording, Some(statistics), Some(draft))
        );
    }

    #[test]
    fn recording_write_invalidates_older_reads_and_pauses_refresh() {
        let mut app = connected_app(MonitorTab::Info);
        let (statistics_sender, statistics_receiver) = unbounded();
        statistics_sender.send(Ok(recorder_statistics_with_total(10).into())).unwrap();
        app.statistics_receiver = Some(statistics_receiver);
        let (_sender, receiver) = unbounded();
        app.finish_start_recording_configuration(receiver, Ok(()));

        app.refresh_with(|| panic!("connected discovery"), |_| panic!("read during write"));
        app.poll_recorder_statistics();

        assert_eq!((app.statistics_receiver.is_none(), app.recorder_statistics), (true, None));
    }

    #[test]
    fn reconnect_discards_previous_statistics_and_write_completion() {
        let mut app = connected_app(MonitorTab::Info);
        app.instances.push(Instance {
            descriptor: descriptor(1),
            recording: RecordingConfiguration::default(),
            server_version: None,
        });
        let (statistics_sender, statistics_receiver) = unbounded();
        statistics_sender.send(Ok(recorder_statistics_with_total(10).into())).unwrap();
        app.statistics_receiver = Some(statistics_receiver);
        let (sender, receiver) = unbounded();
        let mut configuration = RecordingConfiguration::default();
        configuration.io.enabled = true;
        sender
            .send(Ok(RecordingUpdate {
                descriptor: descriptor(1),
                configuration,
            }))
            .unwrap();
        app.finish_start_recording_configuration(receiver, Ok(()));
        // Keep Escape's automatic discovery deterministic and local.
        let (_discovery_sender, discovery_receiver) = unbounded();
        app.discovery_receiver = Some(discovery_receiver);

        app.handle_key(KeyCode::Esc);
        app.handle_key(KeyCode::Enter);
        app.poll_recorder_statistics();
        app.poll_recording_configuration();

        assert_eq!(
            (
                connected_fields(&app.screen).unwrap().1,
                app.recorder_statistics,
                app.status.as_str()
            ),
            (RecordingConfiguration::default(), None, "")
        );
    }

    #[test]
    fn failed_recording_write_schedules_authoritative_refresh() {
        let mut app = connected_app(MonitorTab::Info);
        app.next_refresh = Instant::now() + Duration::from_mins(1);
        let (sender, receiver) = unbounded();
        sender.send(Err("cache update failed after legacy update".into())).unwrap();
        app.finish_start_recording_configuration(receiver, Ok(()));

        app.poll_recording_configuration();

        assert_eq!(
            (
                app.next_refresh <= Instant::now(),
                app.recording_receiver.is_none(),
                app.status.as_str()
            ),
            (true, true, "cache update failed after legacy update")
        );
    }

    #[test]
    fn changing_instances_discards_queued_statistics() {
        let mut app = connected_app(MonitorTab::Info);
        app.instances.push(Instance {
            descriptor: descriptor(2),
            recording: RecordingConfiguration::default(),
            server_version: None,
        });
        let (sender, receiver) = unbounded();
        sender.send(Ok(recorder_statistics_with_total(10).into())).unwrap();
        app.statistics_receiver = Some(receiver);
        let (_discovery_sender, discovery_receiver) = unbounded();
        app.discovery_receiver = Some(discovery_receiver);

        app.handle_key(KeyCode::Esc);
        app.handle_key(KeyCode::Enter);
        app.poll_recorder_statistics();

        assert_eq!(
            (connected_fields(&app.screen).unwrap().0, app.recorder_statistics),
            (descriptor(2).instance_id, None)
        );
    }

    #[test]
    fn recording_worker_uses_readback_instead_of_requested_configuration() {
        let mut app = connected_app(MonitorTab::Info);
        let authoritative = RecordingConfiguration {
            event_capacity_per_thread: 128,
            ..RecordingConfiguration::default()
        };
        app.apply_recording_configuration_with(RecordingConfiguration::default(), move |_, _| Ok(authoritative));
        let receiver = app.recording_receiver.take().unwrap();
        let result = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(result.unwrap().configuration, authoritative);
    }

    #[test]
    fn on_preset_enables_backtraces_without_sampling() {
        let mut configuration = RecordingConfiguration::default();
        configuration.allocations.enabled = true;
        configuration.allocations.capture_backtraces = true;
        configuration.allocations.sampling_one_in = 8;
        let mut popup = RecordingConfigurationPopup::new(configuration);
        RecordingConfigurationField::AllocationRecording.adjust(&mut popup, -1);

        assert_eq!(
            (
                RecordingConfigurationField::AllocationRecording.value(popup),
                popup.configuration().allocations,
            ),
            (
                "on".to_owned(),
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 1,
                },
            )
        );
    }

    #[test]
    fn every_on_preset_records_all_events_with_backtraces_and_reads_back_as_on() {
        let actual = [
            (RecordingConfigurationField::AllocationRecording, RecorderKind::Allocations),
            (RecordingConfigurationField::GeneralRecording, RecorderKind::General),
            (RecordingConfigurationField::ArcDereferenceRecording, RecorderKind::ArcDereferences),
            (RecordingConfigurationField::RuntimeTaskRecording, RecorderKind::RuntimeTasks),
            (RecordingConfigurationField::IoRecording, RecorderKind::Io),
            (RecordingConfigurationField::CacheRecording, RecorderKind::Cache),
        ]
        .map(|(field, recorder)| {
            let mut popup = RecordingConfigurationPopup::new(RecordingConfiguration::default());
            field.adjust(&mut popup, 1);
            let configuration = popup.configuration();
            let readback = RecordingConfigurationPopup::new(configuration);
            (
                recorder_policy(configuration, recorder),
                field.value(readback),
                readback.configuration() == configuration,
            )
        });
        assert_eq!(
            actual,
            std::array::from_fn(|_| (
                seismograph_protocol::message::RecordingPolicy {
                    enabled: true,
                    capture_backtraces: true,
                    sampling_one_in: 1,
                },
                "on".to_owned(),
                true,
            ))
        );
    }

    #[test]
    fn sampled_or_stackless_recording_is_custom() {
        let policies = [
            seismograph_protocol::message::RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                sampling_one_in: 1,
            },
            seismograph_protocol::message::RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 8,
            },
        ];
        assert_eq!(policies.map(recording_policy_label), ["custom", "custom"]);
    }

    #[test]
    fn recording_configuration_worker_start_failure_is_reported() {
        let mut app = connected_app(MonitorTab::Info);
        let (_sender, receiver) = unbounded();

        app.finish_start_recording_configuration(receiver, Err(std::io::Error::other("worker unavailable")));

        assert_eq!(app.status, "failed to start recording configuration worker: worker unavailable");
        assert!(app.recording_receiver.is_none());
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
        assert!(!view.descending);
    }

    #[test]
    fn runtime_activity_tab_always_changes_the_main_tab() {
        for focus in [
            RuntimeFocus::Workers,
            RuntimeFocus::Tasks,
            RuntimeFocus::Activity,
            RuntimeFocus::Events,
        ] {
            let mut app = App::offline("capture.seismograph".into());
            if let Screen::Offline { tab, .. } = &mut app.screen {
                *tab = MonitorTab::Runtime;
            }
            app.runtime_view.focus = focus;
            app.handle_key(KeyCode::Tab);
            assert!(matches!(app.screen, Screen::Offline { tab: MonitorTab::Io, .. }));
            app.handle_key(KeyCode::BackTab);
            assert!(matches!(
                app.screen,
                Screen::Offline {
                    tab: MonitorTab::Runtime,
                    ..
                }
            ));
            assert_eq!(app.runtime_view.focus, focus);
        }
    }

    #[test]
    fn dashboard_keeps_main_tab_navigation_and_independent_chart_and_event_focus() {
        let mut app = connected_app(MonitorTab::Runtime);
        app.runtime_view.focus = RuntimeFocus::Tasks;
        app.runtime_view.activity_scroll = 4;
        app.handle_key(KeyCode::Char('e'));
        assert_eq!(app.runtime_view.focus, RuntimeFocus::Events);
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.runtime_view.events.focus, TaskEventsFocus::Occurrences);
        app.handle_key(KeyCode::Char('f'));
        app.handle_key(KeyCode::PageDown);
        app.handle_key(KeyCode::Right);
        assert_eq!(
            (
                app.runtime_view.events.stack_filter,
                app.runtime_view.events.stack_scroll,
                app.runtime_view.events.stack_horizontal_scroll
            ),
            (AllocationStackFilter::All, 5, 8)
        );
        app.handle_key(KeyCode::Tab);
        assert!(matches!(app.screen, Screen::Connected { tab: MonitorTab::Io, .. }));
        app.handle_key(KeyCode::BackTab);
        app.handle_key(KeyCode::Char('t'));
        assert_eq!(
            (
                app.runtime_view.focus,
                app.runtime_view.task_histogram,
                app.runtime_view.activity_scroll
            ),
            (RuntimeFocus::Activity, TaskHistogram::Ready, 4)
        );
        app.handle_key(KeyCode::Char('e'));
        for _ in 0..3 {
            app.handle_key(KeyCode::Backspace);
        }
        assert_eq!(app.runtime_view.focus, RuntimeFocus::Tasks);
        app.handle_key(KeyCode::Down);
        assert_eq!(
            (
                app.runtime_view.events.event_selected,
                app.runtime_view.events.stack_scroll,
                app.runtime_view.events.stack_horizontal_scroll
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn runtime_keys_cover_navigation_sorting_and_scroll() {
        let mut view = RuntimeViewState::new();
        view.worker_selected = 2;
        view.task_selected = 2;
        view.activity_scroll = 6;
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
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
        assert!(!view.task_sort_descending);

        let original = RuntimeViewState::new();
        let mut unchanged = original;
        assert!(!handle_runtime_key(KeyCode::Tab, &mut unchanged, None));
        assert_eq!(unchanged, original);
    }

    #[test]
    fn io_and_cache_keys_move_between_summary_and_operation_views() {
        let mut io = IoViewState::new();
        let mut cache = CacheViewState::new();
        io.resource_selected = 2;
        cache.tier_selected = 2;
        assert!(handle_io_key(KeyCode::Up, &mut io, None));
        assert!(handle_cache_key(KeyCode::Up, &mut cache, None));
        assert_eq!((io.resource_selected, cache.tier_selected), (1, 1));
        io = IoViewState::new();
        cache = CacheViewState::new();
        for key in [KeyCode::Down, KeyCode::Enter, KeyCode::Down, KeyCode::Up, KeyCode::Backspace] {
            assert!(handle_io_key(key, &mut io, None));
            assert!(handle_cache_key(key, &mut cache, None));
        }
        assert_eq!((io, cache), (IoViewState::new(), CacheViewState::new()));
        assert!(!handle_io_key(KeyCode::Backspace, &mut io, None));
        assert!(!handle_cache_key(KeyCode::Backspace, &mut cache, None));
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
        assert!(matches!(receiver.try_recv().unwrap(), CaptureMessage::Progress(CaptureStep::Save)));
        drop(receiver);
        assert_eq!(
            report_capture_step(&sender, CaptureStep::Save),
            Err("snapshot progress receiver closed".into())
        );
    }

    #[test]
    fn snapshot_sources_are_selected_by_exact_identifier() {
        for (id, expected) in [
            (seismograph_rallocator::source::ID, Some("invalid rallocator snapshot")),
            (seismograph_runtime::snapshot::source::ID, Some("invalid runtime snapshot")),
            (seismograph::snapshot::SourceId::new(999), None),
        ] {
            let decoded = seismograph::snapshot::DecodedSnapshot {
                sources: vec![seismograph::snapshot::SourceSnapshot {
                    id,
                    name: "not-used-for-identification".into(),
                    schema_version: 1,
                    data: Vec::new(),
                }],
                ..Default::default()
            };
            let result = super::super::snapshot::prepare(decoded);
            assert_eq!(
                result.err().map(|error| error.split(':').next().unwrap().to_owned()),
                expected.map(str::to_owned),
            );
        }
    }

    #[test]
    fn offline_initial_status_reports_loading_before_a_snapshot_arrives() {
        let app = App::offline(PathBuf::from("capture.seismograph"));

        assert_eq!(app.status, "Loading snapshot…");
    }

    #[test]
    fn offline_navigation_never_starts_remote_actions() {
        let mut app = App::offline(PathBuf::from("capture.seismograph"));
        app.finish_offline_load(empty_capture());
        for tab in ['1', '2', '3', '4', '5', '6', '7', '8'] {
            app.handle_key(KeyCode::Char(tab));
            for key in ['s', 'r', 'c', 'd', 'C'] {
                assert!(!app.handle_key(KeyCode::Char(key)));
            }
            app.refresh_with(|| panic!("offline discovery"), |_| panic!("offline statistics"));
            app.apply_recording_configuration_with(RecordingConfiguration::default(), |_, _| panic!("offline recording"));
        }
        assert!(matches!(
            app.screen,
            Screen::Offline {
                tab: MonitorTab::Cache,
                ..
            }
        ));
        assert_eq!(
            (
                app.capture_receiver.is_none(),
                app.discovery_receiver.is_none(),
                app.statistics_receiver.is_none(),
                app.recording_receiver.is_none(),
                app.recording_configuration_popup.is_none(),
                app.capture_mode,
            ),
            (true, true, true, true, true, CaptureMode::Continue),
        );
        assert!(app.handle_key(KeyCode::Esc));
        assert!(app.handle_key(KeyCode::Char('q')));
    }

    #[test]
    fn offline_navigation_sorting_and_stack_filters_match_monitor() {
        for tab in [
            MonitorTab::Info,
            MonitorTab::Heaps,
            MonitorTab::Allocations,
            MonitorTab::Primitives,
            MonitorTab::Threads,
            MonitorTab::Runtime,
            MonitorTab::Io,
            MonitorTab::Cache,
        ] {
            let mut live = connected_app(tab);
            let mut offline = App::offline(PathBuf::from("capture.seismograph"));
            offline.screen = Screen::Offline {
                path: PathBuf::from("capture.seismograph"),
                tab,
                snapshot: Some(empty_capture()),
            };
            for key in [
                KeyCode::Char(']'),
                KeyCode::Char('r'),
                KeyCode::Char('f'),
                KeyCode::Down,
                KeyCode::Enter,
                KeyCode::PageDown,
                KeyCode::Backspace,
                KeyCode::Tab,
                KeyCode::BackTab,
            ] {
                assert_eq!(live.handle_key(key), offline.handle_key(key));
                let state = |app: &App| {
                    (
                        app.heap_view,
                        app.allocation_view,
                        app.primitive_view,
                        app.thread_view,
                        app.runtime_view,
                        app.io_view,
                        app.cache_view,
                    )
                };
                assert_eq!(state(&live), state(&offline), "{tab:?} {key:?}");
            }
        }
    }
}

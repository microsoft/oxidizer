// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::help::Context;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Section {
    pub(super) heading: &'static str,
    pub(super) entries: &'static [Entry],
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Entry {
    pub(super) keyword: &'static str,
    pub(super) explanation: &'static str,
}

macro_rules! section {
    ($heading:literal; $($keyword:literal => $explanation:literal),+ $(,)?) => {
        &Section {
            heading: $heading,
            entries: &[$(Entry { keyword: $keyword, explanation: $explanation }),+],
        }
    };
}

pub(super) const fn title(context: Context) -> &'static str {
    match context {
        Context::Browser => "Applications",
        Context::Info => "Info and activity",
        Context::HeapBuckets => "Native v4: global backend, owners and coverage",
        Context::HeapHotspots => "Native v4: owner structures and returns",
        Context::Allocations => "Allocation hotspots and stack",
        Context::PrimitiveTypes => "Primitive types",
        Context::PrimitiveOperations => "Primitive operations",
        Context::PrimitiveHotspots => "Primitive hotspots and stack",
        Context::Threads => "Threads",
        Context::ThreadOperations => "Thread operations",
        Context::ThreadParticipants => "Related threads",
        Context::ThreadObjects => "Thread objects and stacks",
        Context::RuntimeWorkers => "Runtime workers",
        Context::RuntimeTasks => "Runtime tasks",
        Context::RuntimeActivity => "Runtime task details",
        Context::RuntimeEvents => "Inferred task events",
        Context::IoResources => "I/O resources",
        Context::IoOperations => "I/O operations",
        Context::CacheTiers => "Cache tiers",
        Context::CacheOperations => "Cache outcomes",
        Context::Recording => "Recording configuration",
        Context::Filters => "Stack filters",
        Context::Capture => "Capturing a snapshot",
        Context::Loading => "Loading a snapshot",
        Context::Error => "Snapshot errors",
    }
}

pub(super) fn document(context: Context, offline: bool) -> Vec<&'static Section> {
    let sections: &[&Section] = match context {
        Context::Browser => &[BROWSER],
        Context::Info => {
            if offline {
                &[OFFLINE_INFO, CAPTURE_SCOPE]
            } else {
                &[INFO, ACTIVITY, LIVE_THREADS, CAPTURE_SCOPE]
            }
        }
        Context::HeapBuckets | Context::HeapHotspots => &[HEAP_BUCKETS, HEAP_SUMMARY, HEAP_HOTSPOTS],
        Context::Allocations => &[ALLOCATIONS, STACK],
        Context::PrimitiveTypes => &[PRIMITIVE_TYPES],
        Context::PrimitiveOperations => &[PRIMITIVE_OPERATIONS],
        Context::PrimitiveHotspots => &[PRIMITIVE_HOTSPOTS, STACK],
        Context::Threads => &[THREADS],
        Context::ThreadOperations => &[THREAD_OPERATIONS],
        Context::ThreadParticipants => &[THREAD_PARTICIPANTS],
        Context::ThreadObjects => &[THREAD_OBJECTS, STACK],
        Context::RuntimeWorkers => &[RUNTIME_WORKERS],
        Context::RuntimeTasks => &[RUNTIME_TASKS],
        Context::RuntimeActivity => &[RUNTIME_ACTIVITY],
        Context::RuntimeEvents => &[RUNTIME_EVENTS],
        Context::IoResources => &[IO_RESOURCES],
        Context::IoOperations => &[IO_OPERATIONS],
        Context::CacheTiers => &[CACHE_TIERS],
        Context::CacheOperations => &[CACHE_OPERATIONS],
        Context::Recording => &[RECORDING],
        Context::Filters => &[FILTERS],
        Context::Capture => &[CAPTURE],
        Context::Loading => &[LOADING],
        Context::Error => &[ERROR],
    };
    let mut document = sections.to_vec();
    if matches!(context, Context::PrimitiveOperations | Context::ThreadOperations) {
        document.push(PRIMITIVE_VALUES);
    }
    if matches!(context, Context::RuntimeTasks | Context::RuntimeActivity) {
        document.push(RUNTIME_STATES);
    }
    if matches!(context, Context::Info) {
        document.extend([VERSIONS, COMMON, if offline { OFFLINE } else { LIVE }]);
    }
    document.push(HELP_CONTROLS);
    document
}

const VERSIONS: &Section = section!("CRATE VERSIONS";
    "Monitor (seismograph_cli)" => "Semantic version compiled into this local monitor executable. Shown in the Info view for live connections and saved snapshots.",
    "Server (seismograph)" => "Semantic version of the seismograph crate in the connected application, queried during discovery after authentication, before any capture. Shown in the Info view. This is not the protocol crate version or the application's own version.",
    "unknown (legacy server)" => "The server closed the version query without a response. Older servers do not report their version; no version is inferred from the local monitor.",
    "unknown (not recorded in snapshot)" => "Saved snapshots do not contain the server crate version. The local monitor version is not the snapshot producer's version.",
);

const BROWSER: &Section = section!("APPLICATIONS";
    "Application / instance" => "Discovered application name, with an optional instance name in parentheses.",
    "pid" => "Operating-system process ID of the application.",
    "off / on / custom" => "off: all recorder groups disabled. on: every enabled group has backtraces and sampling 1/1; not necessarily every group is enabled. custom: an enabled group has other settings.",
    "Up/Down" => "Select an application.",
    "Enter / click row" => "Connect to the selected application.",
    "r" => "Refresh discovery; discovery also refreshes automatically.",
    "No applications" => "No discoverable instrumented process, not proof that the machine is idle.",
    "q" => "Quit the browser after closing help.",
);

const INFO: &Section = section!("INFO / SOURCE";
    "Name" => "Connected application name.",
    "Instance" => "Application-supplied instance name; '-' means absent.",
    "PID" => "Operating-system process ID.",
    "Monitor port" => "Port serving the application's monitor connection.",
    "Event ring buffer" => "Capacity in events per recording thread.",
    "Allocations / General events / Arc dereferences / Runtime tasks / I/O / Cache" => "Independent recording policies. Each shows enabled state, backtraces on/off, and sample 1/N (percentage = 100/N).",
    "Telemetry memory / thread" => "Ring-buffer memory for the configured capacity, not application heap usage.",
    "Telemetry threads" => "Number of recorder threads.",
    "Telemetry memory total" => "Allocated recorder storage from live statistics. Before statistics arrive, estimated as per-thread memory times captured source thread count.",
    "Accepted telemetry events" => "Cumulative accepted records, excluding disabled, suppressed and sampled-out activity.",
    "Retained telemetry events" => "Records still stored in the source buffers.",
    "Overwritten telemetry events" => "Records displaced by ring reuse.",
    "All-class source accepted / overwritten" => "Fallback counts from the captured allocation source before live statistics arrive. They span event classes, not just allocations.",
    "Retained allocator events" => "Fallback retained count covers allocator records only; its scope differs from all-class source counters.",
);

const ACTIVITY: &Section = section!("LIVE ACTIVITY";
    "events/s" => "Change in accepted telemetry events divided by elapsed sample time; not requests/s or CPU utilization.",
    "total" => "Latest cumulative accepted-event counter.",
    "Colors / classes" => "Cyan: allocations and frees. Green: general primitive events. Magenta: Arc dereferences. Yellow: runtime tasks. Light blue: I/O. Light red: cache. Each legend value is that class's current accepted-event rate across recording threads.",
    "Graph" => "Up to 120 approximately one-second samples. Horizontal axis: elapsed time in seconds. Vertical axis: events/second on one shared automatically scaled maximum. The legend keeps exact integer rates even when small series look flat beside larger ones.",
    "First sample / zero" => "A first observation or changed recording session establishes a baseline and shows '-' rather than an invented rate. Later zero means no new accepted records in the measured interval, not zero application work. Counter resets never produce wraparound spikes.",
    "Stale / unsupported" => "Failed polling marks graphics stale and hides current rate labels until counters arrive again. Older servers retain the aggregate rate graph but explicitly mark thread/class counters unavailable. Offline files never produce live rates.",
    "Live versus snapshot" => "Statistics refresh independently. A graph change does not refresh the manually captured analysis tables.",
);

const LIVE_THREADS: &Section = section!("LIVE THREAD ACTIVITY";
    "Thread / name" => "Recorder thread ID and captured name, not an inferred task or an OS-wide thread census. All available threads from the recording session are browsable, including exited threads retained by the recorder.",
    "Fill bar" => "Currently retained events / that thread's actual ring capacity, not the configured capacity for future rings. Red means the ring is full; subsequent records overwrite older records. A released ring is labeled explicitly, not shown as 0/0 percent.",
    "events/s / sparkline" => "Per-thread accepted counter delta divided by elapsed observation time. Sparklines use up to 120 recent samples, independently scaled to each thread's visible maximum; their heights are not comparable between threads. Gray '_' means measured zero, not missing data.",
    "Up/Down / PgUp/PgDn / Home/End" => "Move through all threads, page eight at a time, or go to the first/last thread. Each thread occupies one line: identity, fill bar, rate and sparkline. Mouse wheel scrolls over the thread pane; clicking a line selects that exact thread.",
    "Scope / cost" => "Polling reads lightweight counters, not event payloads, backtraces, or snapshot sources. Disabled, suppressed, and sampled-out events are excluded. Counts and fill are live observations across independently running threads, not a globally atomic snapshot. Snapshot filters do not alter these counters.",
);

const CAPTURE_SCOPE: &Section = section!("CAPTURE METRIC SCOPE";
    "Accepted / overwritten" => "Counts exclude suppressed, sampled-out, disabled and non-producing activity. Source accepted/overwritten counters span event classes; they are not allocation populations.",
    "Unmatched allocations" => "Unmatched retained allocations are not proven live allocations or leaks, even with zero overwrites.",
    "Native capacity" => "Outstanding native ranges and slab capacity are not application-live memory. Global caches are not guaranteed physically decommitted.",
    "Native coverage" => "Unobserved and busy owners are unknown, not zero. Lease/session/round freshness is explicit; bounded inventories can be partial.",
    "Native returns" => "Remaining batching budget is not pending bytes; sampled incoming front != back means potential work only, not guaranteed ready links or queue depth. Equality does not prove emptiness.",
);

const OFFLINE_INFO: &Section = section!("SNAPSHOT FILE";
    "Path" => "Native .seismograph file being inspected.",
    "Capture time" => "Capture time is not recorded in this format; loading time is not capture time.",
    "Source events" => "All-class accepted and overwritten source counts, plus the number of thread summaries; not an allocation population or a live rate.",
    "Heap errors" => "Heap decoding may fail while other telemetry remains available.",
    "Offline" => "There is no live activity polling or process connection. Scope notes apply to saved data.",
);

const HEAP_SUMMARY: &Section = section!("NATIVE GLOBAL BACKEND / COVERAGE";
    "Reservations" => "Cumulative successful native object-range reservations, excluding the pagemap; not retained or application-live bytes.",
    "Pagemap VA" => "Sparse pagemap virtual address reservation, not resident or committed memory.",
    "Growth" => "Local range-cache boundary and global refill ceiling. Individual requests can exceed the refill ceiling.",
    "Global cached range capacity" => "Power-of-two cached ranges. Physical residency is unknown: discard failures prevent a guaranteed decommit claim.",
    "Owners" => "Captured endpoints / total inventoried endpoints. PARTIAL means bounded collection omitted owners.",
    "Current / idle inspected / unknown / busy" => "Same-round published state, fresh collector inspection, missing observations, and busy slots. Busy overlaps observation freshness.",
    "Session / round / capture" => "Recorder session, requested observation round and process-relative monotonic time. Not a globally atomic snapshot.",
    "Publication" => "Self-publication defaults enabled, but only accepted recorded allocation/free events can trigger it. Recording remains off by default.",
    "Filters" => "Native owner/backend inventory remains unfiltered.",
);

const HEAP_BUCKETS: &Section = section!("NATIVE OWNER STRUCTURES";
    "Endpoint / lease" => "Persistent native endpoint address and current lease generation. Active means leased, not attributed to a known current thread.",
    "Freshness" => "Contributed this round, fresh idle inspection, stale lease/session, older round, newer than capture, busy, unobserved or Unavailable (System publication-slot allocation failed). Matching rounds are not an exact current census; a first round can predate polling. Age is shown at capture. Missing state is unknown, not zero.",
    "Publication coverage" => "Only accepted recorded allocation/free operations publish after the native operation completes. Sampled-out events and merely enabled attempts do not contribute. Polling requests the next round after collection; explicit app requests also work. No timers or background workers.",
    "Last contributor" => "Recorder thread that last published this observation. It is not the current lease holder, especially for reused leases.",
    "Object / slab / capacity" => "Rounded object size, one slab's backing bytes and one slab's object capacity; not app-live memory.",
    "Available / empty / observed / fast" => "Native available-list slabs, reusable empty slabs, slabs found by bounded walks, and whether the fast list has an object.",
    "Bars" => "Observed slab counts relative to the largest class count, or range counts relative to the largest bin. They are not utilization gauges.",
    "Large outstanding" => "Native outstanding large ranges, not application-live ranges. Queued remote returns can still be outstanding.",
    "Local backend / metadata" => "Reusable object and metadata ranges by base-two size exponent. Incomplete walks show observed capacity only.",
    "Requested growth" => "Cumulative local refill growth state, not retained bytes.",
    "Up/Down / Home/End" => "Select global backend or an owner, or jump to the first/last row. Mouse clicks select the exact rendered owner.",
    "PgUp/PgDn" => "Scroll the selected structure detail.",
);

const HEAP_HOTSPOTS: &Section = section!("NATIVE RETURNS / LIMITATIONS";
    "Outgoing returns" => "Open rings/objects and bounded message-bucket/list walks, including class-rounded message bytes. PARTIAL means nodes were omitted.",
    "Batching budget" => "Remaining native batching budget, NOT pending or queued bytes.",
    "Incoming atomic queue" => "Independently sampled front/back addresses, never dereferenced or traversed. Queue depth and emptiness remain unknown.",
    "Capture consistency" => "Owners and the global backend are independent observations. Active slots may be busy, unobserved or stale.",
    "Missing observations" => "Never-observed endpoints remain visible; they are not zero-capacity owners.",
);

const ALLOCATIONS: &Section = section!("ALLOCATION HOTSPOTS";
    "Allocations" => "Retained allocation-event count for this captured stack.",
    "Allocated" => "Sum of requested allocation bytes.",
    "Average" => "Allocated / Allocations in integer bytes; zero when the count is zero.",
    "Unmatched" => "Retained allocation count without a matched retained free.",
    "Unmatched B" => "Corresponding unmatched bytes. Neither unmatched metric proves process-live allocations or leaks, even with zero overwrites.",
    "Location" => "First displayed frame. Stack Trace shows this group's captured call stack.",
    "All-class source accepted / overwritten" => "Source counts spanning event classes, not allocation populations.",
    "Sampling / recording boundaries" => "Can omit a matching free and break live-allocation inference.",
    "Up/Down" => "Select a hotspot.",
    "[ / ] / r" => "Change sort column / reverse sort.",
    "f / PgUp/PgDn" => "Toggle application/all frames / scroll the stack.",
    "e" => "Switch hotspots / individual allocation-free records. Records include actor names, requested sizes, addresses, operation stacks and view-local lifetime IDs.",
    "Orphan free" => "No retained allocation origin in the original capture. Filtering does not create orphan status or recompute lifetime IDs.",
    "Repeated addresses" => "View-local IDs distinguish each retained lifetime. Source keys/addresses may repeat and missing events prevent global lifetime reconstruction.",
);

const STACK: &Section = section!("STACK TRACE";
    "Frame number" => "Zero-based frame index, not a count or duration.",
    "Frame text" => "Resolved symbol and available source location, or an unresolved address.",
    "Backtraces disabled / not captured" => "Attribution is unavailable, not proof that no work occurred. Stack capture must be enabled when the event happens.",
    "f" => "Presentation only: application frames versus all frames. Does not change record counts.",
    "F" => "Whole-record stack filters, distinct from lowercase f.",
    "PgUp/PgDn" => "Scroll the selected stack after closing help.",
);

const PRIMITIVE_TYPES: &Section = section!("PRIMITIVE TYPES";
    "Type" => "Arc: shared reference-counted ownership. Mutex: exclusive access. RwLock: shared readers/exclusive writer. Barrier: group synchronization. Condvar: notification wait. OnceLock / LazyLock: one-time initialization. Channel: message passing.",
    "Events" => "Retained primitive-event count.",
    "Objects" => "Distinct recorded object identities in this family.",
    "Contentions" => "Count of event kinds classified as contention, not nanoseconds blocked. Channel receive wait is normal waiting for a producer and is excluded.",
    "retained primitive / source accepted / overwritten" => "Retained primitive is class-specific; source counters span all event classes.",
    "Up/Down / Enter" => "Select a type / enter Operations.",
);

const PRIMITIVE_OPERATIONS: &Section = section!("PRIMITIVE OPERATIONS";
    "Operation" => "Instrumented event kind for the selected primitive type.",
    "Events" => "Retained event count for this operation.",
    "Objects" => "Distinct associated object identities.",
    "Threads" => "Distinct emitting recorder threads.",
    "Hotspots" => "Captured stack-group count.",
    "Highlighted contention" => "A recorded contention kind, not a latency or CPU measurement. Receive wait (empty) is normal channel waiting.",
    "Up/Down / Enter / Backspace" => "Select operation / enter Hotspots / go up.",
    "[ / ] / r" => "Sort by Events, Objects, Threads or Hotspots / reverse.",
);

const PRIMITIVE_HOTSPOTS: &Section = section!("PRIMITIVE HOTSPOTS";
    "Events" => "Retained records sharing this stack for the selected type and operation.",
    "Location" => "First displayed frame of the selected hotspot.",
    "Stack Trace" => "Selected hotspot's stack, not every object of the primitive.",
    "Up/Down / Backspace" => "Select hotspot / return to Operations.",
    "f / PgUp/PgDn" => "Toggle application/all frames / scroll the stack.",
);

const PRIMITIVE_VALUES: &Section = section!("OPERATION VALUES";
    "Arc: Create / Clone / Deref" => "Creation, reference cloning, and dereference events; not the current reference count.",
    "Arc: Final drop / Relocate" => "Final-owner destruction / relocation.",
    "Mutex: Acquisition / Release / Contention" => "Acquiring exclusive access / releasing it / waiting to acquire it.",
    "RwLock: Read acquisition / Read release / Read contention" => "Acquiring shared access / releasing it / waiting for it.",
    "RwLock: Write acquisition / Write release / Write contention" => "Acquiring exclusive access / releasing it / waiting for it.",
    "Barrier / Condvar: Completed wait / Blocked wait" => "A wait returning / a wait that blocked.",
    "Generation release / Notification" => "Barrier generation released / Condvar waiters signaled.",
    "Once: Access / Initialization / Contention" => "Using the value / initializing it / waiting for initialization.",
    "Channel: Send / Receive" => "Recorded message-transfer operations.",
    "Send contention / Receive wait (empty)" => "Blocked send / normal waiting for a message. Receive waiting is excluded from Contentions.",
    "Close / High watermark" => "Channel closing / high-watermark EVENTS. The Events column counts occurrences, not the queue depth value.",
    "Poisoned / Poison observed / Poison cleared" => "Lock becomes poisoned / a caller observes poison / poison is cleared. Not contention durations.",
    "Allocation / Deallocation" => "Threads also shows creation/free records; primitive operation names there include family prefixes.",
);

const THREADS: &Section = section!("THREADS";
    "Thread" => "Recorder thread ID and available name, not necessarily OS thread ID. Sorted by recorder ID.",
    "Retained" => "Retained source records for this thread; filtering can reduce this count.",
    "Overwritten" => "All-class source lost-record count for this thread; remains unfiltered.",
    "Up/Down / Enter" => "Select a thread / enter its Operations.",
    "Empty operations" => "Other event classes can contribute retained events without relevant operation rows.",
);

const THREAD_OPERATIONS: &Section = section!("THREAD OPERATIONS";
    "Operation" => "Selected thread's recorded operation category.",
    "Events" => "Its retained records in that category.",
    "Objects" => "Distinct associated recorded identities.",
    "Threads" => "Related participant rows, not all process threads.",
    "Relationships" => "Association through retained object identities, not a scheduler wait-for graph or proof one thread blocked another. Allocation/free operations connect their counterparts.",
    "Channel receive wait" => "Often normal producer/consumer behavior, not a bug.",
    "Up/Down / Enter / Backspace" => "Select operation / enter Related Threads / return to Threads.",
);

const THREAD_PARTICIPANTS: &Section = section!("RELATED THREADS";
    "Thread" => "Related recorder thread; (self) is the originally selected thread.",
    "Objects" => "Shared recorded objects in this relationship.",
    "Events" => "Participant's retained events on those objects.",
    "Relationship title" => "Allocation/free counterparts or primitive users, including self. Shared identity does not establish causality, wall-clock overlap or lock ownership.",
    "Up/Down / Enter / Backspace" => "Select participant / enter Objects / return to Operations.",
);

const THREAD_OBJECTS: &Section = section!("THREAD OBJECTS / STACKS";
    "Object" => "Hexadecimal recorded identity, not a size or portable pointer.",
    "Own" => "Selected thread's retained events for this operation/object.",
    "Related" => "Participant's corresponding retained events.",
    "Hotness" => "Ranked using Own + Related, not elapsed time.",
    "Selected thread operation / Related thread operation" => "Representative captured stack on each side, not a complete chronological trace.",
    "event(s)" => "Retained count for that representative stack, not the entire object's event count.",
    "Missing stacks" => "Backtrace settings and partial retention can remove attribution on either side.",
    "Up/Down / Backspace" => "Select object / go up.",
    "f / PgUp/PgDn" => "Toggle application/all frames / scroll both stack sections.",
);

const RUNTIME_WORKERS: &Section = section!("RUNTIME WORKERS";
    "Runtime / thread" => "Runtime ID:worker ID / recorder-thread ID, followed by the runtime name. A '?' thread is unknown, not the notifier's OS thread.",
    "Role" => "Core executes general tasks; Blocking executes blocking work; Io drives runtime I/O.",
    "State" => "Running means available to execute, not necessarily polling now. Parked means parked; Stopped means stopped. Event-only workers can have blank metadata.",
    "Tasks" => "Distinct tasks observed polling on this worker, not all source-associated tasks. A task can be listed below with zero observed polls. Migrated tasks can appear on multiple workers.",
    "Polls" => "Retained completed polls on this worker, not a lifetime count. An ongoing poll contributes to the timeline, not this count.",
    "Median poll" => "Median retained completed poll duration across this worker's tasks; '-' without samples and 0ns for measured zero.",
    "Max poll" => "Longest retained completed poll duration on this worker; an ongoing poll is not a completed sample.",
    "Observed %" => "Union of observed poll intervals, clipped to the displayed per-runtime window, divided by that same window's duration. A lower bound, never above 100%; NOT CPU utilization. No valid window or no observation is '-', not zero.",
    "Poll timeline" => "Each row bins that worker's observed polls on its runtime's common time window. Eight rising bar heights show occupied fraction (0..100%). Every positive fraction rises above the zero baseline. Dim baseline bars are unobserved, not proof of idle. Different runtimes may have different windows.",
    "Worker Activity" => "Selected worker's window axis, observed poll timeline and completed poll duration histogram. Duration runs horizontally across 12 fixed log10 buckets; count is vertical. Every nonempty bucket has at least a tiny visible sliver. The selected bucket's range and exact count appear above the bars. Open source polls contribute only to the timeline and observed fraction.",
    "unassigned / Unbound" => "Tasks with no worker association. '-' poll metrics are unavailable, not zero.",
    "runtime events / source retained" => "Retained runtime-class count / retained all-class source-event count.",
    "accepted / overwritten / loss %" => "All-class source totals. Loss percentage = overwritten / accepted; zero when accepted is zero.",
    "No runtime data" => "A recorder does not instrument executors automatically. Runtime source presence alone is not event history.",
    "Up/Down / Enter" => "Select worker / enter Tasks. Backspace returns from Tasks. On narrow terminals only the focused panel is shown.",
);

const RUNTIME_TASKS: &Section = section!("RUNTIME TASKS";
    "Task" => "Runtime task ID.",
    "Future B" => "Inline size of the submitted future, in exact bytes; synchronous tasks use their closure size. Excludes executor bookkeeping, runtime wrappers and separately allocated buffers. Zero is a genuinely zero-sized body; '-' means unavailable in the producer or an older recording.",
    "State(global)" => "Task-global coherently observed source activity, not execution on this historical worker; unknown for legacy or raced source state. Source-associated tasks remain visible without any completed retained polls.",
    "Polls" => "Number of retained completed polls on THIS selected worker. Zero completed polls displays 0; not a lifetime counter.",
    "Observed %" => "Union of this task's observed polls on THIS worker / the same displayed per-runtime window duration. Open coherent source polls can contribute. Lower bound, not CPU utilization; unavailable is '-'.",
    "Median poll" => "Median completed poll duration on THIS worker. Even populations use the midpoint rounded down to nanoseconds. '-' means no samples, while 0ns is a genuine measured zero.",
    "Max poll" => "Largest completed retained poll duration on THIS worker; '-' without samples. Open polls are not completed duration samples.",
    "Units / zero" => "Durations use ns/us/ms/s; histogram ranges use inclusive nanoseconds. Zero completed polls is 0. Missing durations are '-', never an invented zero.",
    "Up/Down / Enter / Backspace" => "Select task / focus Statistics / return to Workers. Press e to focus the task's operations below, or t to switch its Poll/Ready histogram.",
    "[ / ] / r" => "Cycle Task, Future B, Polls, Observed %, Median poll and Max poll sorts / reverse direction. Unknown values stay last in either direction. Task selection and detail positions reset.",
);

const RUNTIME_ACTIVITY: &Section = section!("RUNTIME ACTIVITY";
    "Running for / Ready for" => "Age at the coherent per-task source observation. Running is inside a poll; Ready is queued and eligible for a poll. Long ages are shown prominently even without a completed poll. '-' means unavailable, not zero.",
    "Source observation" => "Per-task coherent observations, NOT a globally atomic snapshot of every worker/task. Open execution uses the coherent poll-worker ID, never last-worker metadata or a notifier thread. Older activity schemas without that ID need an exact retained runtime/task/poll-start timestamp match to assign worker occupancy; otherwise only the global running age is shown. Recording stop freezes observation time and ages; viewing or refreshing the stopped snapshot does not advance them. Legacy captures and raced observations have unknown activity rather than guessed ages.",
    "repoll" => "A wake during a running poll requests another poll. This is not yet queue waiting: for a self-wake, ready wait starts at poll finish, not at the raw wake timestamp.",
    "Poll (worker)" => "Observed execution intervals on this selected worker, including an open coherent source poll. The timeline shares the displayed per-runtime nanosecond window with worker charts.",
    "Ready (global)" => "Task-global ready-to-next-poll intervals, possibly across workers. Ready wait is queue waiting, distinct from poll execution, ordinary pending time and raw wake latency. Do not add this task-global metric to worker execution.",
    "Window / baseline" => "All charts for the same runtime use one displayed window, at most the latest 60 seconds of retained event/source observations, and clip interval unions to it. Filtering preserves this original axis. Eight rising bar heights show occupied fraction: zero stays on the baseline and every positive fraction rises above it. Dim baseline bars mean unobserved, not measured idle. Occupancy denominator is end minus start, not summed task lifetimes. Counts/histograms include all retained completed samples; open ages are not truncated to 60 seconds.",
    "Duration (log10 buckets) / count" => "Duration runs left to right across 12 fixed decades: 0..<10ns, 10..<100ns, 100ns..<1us, and so on through milliseconds and seconds, ending in 100s+ (including overflow). Counts run vertically; one-character bars scale linearly to the largest bucket, with a minimum one-eighth-cell sliver for every nonempty bucket. Each bucket has a gray '_' zero baseline, including empty buckets. Numeric counts remain exact. All buckets fit at once; extremely narrow panels merge adjacent buckets. The selected bucket's lower-inclusive, upper-exclusive range and exact count appear above the bars. Axis endpoints use ns/us/ms/s.",
    "Completed only" => "Poll histograms use completed worker-local polls; Ready wait histograms use completed task-global waits explicitly tagged as coherent queue duration. Open source intervals contribute to timelines/observed fractions but are excluded from BOTH duration histograms. No samples displays '-', never a synthetic zero-duration observation.",
    "Wake-to-poll (raw)" => "When no coherent completed queue samples exist, the Ready histogram shows available legacy raw wake-to-poll latency instead. This task-global latency includes self-wake overlap with the preceding Running poll; it is NOT pure scheduler queue time. It never populates the Ready timeline or outstanding Ready age, which continue to use coherent queue evidence. Raw and pure-queue samples are never mixed.",
    "Left/Right / PgUp/PgDn" => "In Statistics, select the previous/next bucket or move five buckets. Up/Down also move one bucket. Click a thin bar or its baseline to inspect its duration range and count. This changes presentation only, not the window or metrics.",
    "Backspace / Enter" => "Enter drills Workers -> Tasks -> Statistics -> Operations -> Occurrences; Backspace reverses that path. Narrow terminals show the focused panel. Wide terminals keep worker activity and Statistics beside the task table, with operations, task-only occurrences and stack across the bottom.",
    "t / Poll / Ready" => "Switch the task duration histogram between worker-local completed polls and task-global completed ready waits; click Poll/Ready for direct selection. F1 explains the metrics.",
    "e" => "Focus the persistent lower task-operation browser without hiding the dashboard.",
    "Tab / Shift-Tab" => "Always cycle main tabs, including from task events.",
    "F filters" => "Event-stack or spawn-provenance filters change retained evidence. Spawn attribution is not evidence of execution at that site. Source-only association does not invent completed poll samples.",
);

const RUNTIME_EVENTS: &Section = section!("INFERRED TASK EVENTS";
    "Inferred" => "Viewer-only correlation with retained runtime poll boundaries on the actual recorder thread. Counts aggregate this task across worker migrations. No task tags are added to event recording; inference is not proof of complete execution history.",
    "Unassigned / ambiguous" => "Missing or inconsistent boundaries and overlapping execution cannot establish a unique actor. Such events are not counted as this task's activity. Captured task state or last-worker association alone is not attribution evidence.",
    "Operations / Occurrences / Stack" => "Three persistent columns: select an operation on the left, one of this task's matching occurrences in the center, and inspect its stack on the right. Up/Down selects; Enter focuses occurrences. Backspace returns to operations, then Statistics. Other actors and other operation kinds are excluded, even when they share the same object.",
    "Time / thread / sequence / object" => "Occurrences are ordered by original nanosecond timestamp, then recorder thread and sequence. Rows show when and where this task performed the selected operation, across all of its workers and matching objects. These row counts match the selected operation's event count.",
    "Allocations" => "An allocation event identifies its inferred allocating task; its freeing actor can differ and is not included unless it is this task. An unmatched allocation is not proven live, and allocations minus frees is not task-owned memory.",
    "Task-relative stack / f" => "Application frames are trimmed using retained poll stacks or a recognized instrumented future wrapper in the event stack. This also works when poll hooks omit backtraces. Without a known boundary the stack is explicitly untrimmed. Press f for the complete captured stack, including executor frames; PgUp/PgDn scrolls vertically and Left/Right pans long frames. Spawn provenance is not an execution stack.",
    "Object identity" => "History notes distinguish stable allocation IDs from same-family address observations. Reused lock or channel addresses do not prove a single object lifetime. Retained creation/destruction boundaries separate Arc observations where available.",
    "Filters" => "Filtering selects displayed records, but inference uses the original poll evidence so hiding a boundary cannot reassign another task's events. Missing captures cannot be recovered by filtering.",
    "e / t" => "e focuses these events; t focuses Statistics and toggles Poll/Ready. Wide layouts keep both visible. Main-tab shortcuts remain available.",
);

const RUNTIME_STATES: &Section = section!("TASK STATE VALUES";
    "Spawned" => "A retained spawn event assigned the task identity.",
    "Materialized" => "A retained task-materialization event was observed.",
    "Running" => "A coherent source activity observation is inside a poll; Running for is its age.",
    "Ready" => "A coherent source activity observation is queued and eligible to poll; Ready for is its queue age.",
    "Waiting" => "An observed poll exit with no outstanding wake. This is not proof of Poll::Pending: the hook does not expose the poll result. Waiting or Ready can briefly appear before terminal retirement after completion or panic.",
    "Completed" => "A retained successful completion transition.",
    "Canceled" => "A retained cancellation transition.",
    "Panicked" => "A retained panic transition.",
    "Unknown / incomplete state" => "No coherent source activity, legacy capture, or raced observation. Retained event gaps cannot prove current readiness or running age.",
);

const IO_RESOURCES: &Section = section!("I/O RESOURCES";
    "Resource" => "Recorded resource ID; resources rank by completed bytes, then event count.",
    "Kind" => "File, TCP stream/listener, Named pipe, WinHTTP, Other or Unknown.",
    "Reads" => "Retained read-start events, not necessarily completed reads.",
    "Writes" => "Retained write-start events, not necessarily completed writes.",
    "Requested" => "Sum of requested bytes from retained starts.",
    "Completed" => "Sum of completed bytes from retained finishes.",
    "Errors" => "Error AND canceled finish events.",
    "Partial windows" => "Start/finish retention can differ. Completed may exceed Requested; rows can outnumber start counts. These are counts/bytes, not throughput.",
    "Source counts" => "Retained counts I/O-class events; accepted/overwritten and loss percentage are all-class source counters.",
    "Up/Down / Enter" => "Select resource / enter Operations.",
);

const IO_OPERATIONS: &Section = section!("I/O OPERATIONS";
    "Operation" => "Recorded operation ID for the selected resource, newest first.",
    "Type" => "Read or Write.",
    "Outcome" => "Latest retained status: pending (no observed completion), success, end of stream, canceled, error, or unknown. A successful read need not fill the buffer.",
    "Thread" => "Latest retained event's recorder thread, not necessarily initiating or OS thread.",
    "Requested" => "Requested bytes from that event.",
    "Completed" => "Completed bytes from that event.",
    "Duration" => "Retained finish minus retained start, in ns/us/ms/s. '-' means the pair is incomplete, not zero latency.",
    "buffer / span(s) / resource" => "Bottom details: buffer ID ('-' if absent), length in bytes, span count and resource kind. Multiple spans describe a logical buffer, not separate completed operations.",
    "Pending / missing duration" => "May reflect a truncated or filtered window, not only an operation still running.",
    "Up/Down / Backspace" => "Select operation / return to Resources.",
);

const CACHE_TIERS: &Section = section!("CACHE TIERS";
    "Tier identity" => "Hexadecimal recorded identity. The same identity can have separate primary/fallback rows.",
    "Role" => "primary or fallback.",
    "Events" => "All retained cache outcomes in this row.",
    "Hits" => "Hit + Refresh hit.",
    "Misses" => "Miss + Expired + Refresh miss + Compute returned none.",
    "Errors" => "Get/Insert/Invalidate/Clear/Refresh errors + Compute failed + Promotion failed.",
    "Hit rate" => "Hits / (Hits + Misses + Errors), as a percentage; '-' when denominator is zero. Not necessarily application request hit ratio: errors include non-lookup operations and requests can emit multiple outcomes.",
    "Source counts" => "Retained counts cache events; accepted/overwritten and loss percentage are all-class source counters.",
    "Up/Down / Enter" => "Select tier / enter Outcomes.",
);

const CACHE_OPERATIONS: &Section = section!("CACHE OUTCOMES";
    "Outcome" => "Instrumented result kind for the selected tier identity and role.",
    "Events" => "Retained count of this outcome, not unique keys, bytes or duration.",
    "Hit / Miss / Expired / Get error" => "Lookup result, expired value, or lookup failure.",
    "Inserted / Insert rejected / Insert error" => "Insertion accepted, rejected, or failed.",
    "Invalidated / Invalidate error" => "Targeted removal succeeded or failed.",
    "Cleared / Clear error / Evicted" => "Whole-cache clear succeeded/failed, or eviction occurred.",
    "Refresh hit / Refresh miss / Refresh error" => "Refresh lookup result.",
    "Refresh suppressed" => "Refresh skipped/suppressed; not automatically an error.",
    "Compute succeeded / Compute failed / Compute returned none" => "Computed value produced, computation failed, or no value produced.",
    "Promotion accepted / Promotion rejected / Promotion failed" => "Promotion between tiers accepted, rejected, or failed. Rejection is not automatically an error.",
    "Up/Down / Backspace" => "Select outcome / return to Tiers.",
);

const RECORDING: &Section = section!("RECORDING CONFIGURATION";
    "Allocations / Events / Arc / Runtime tasks / I/O / Cache" => "Independent recorder groups.",
    "off" => "Disable the selected recorder group.",
    "on" => "FULL recording: enabled + backtraces on + sampling 1 (100%).",
    "custom" => "Expose Backtraces and Sampling options. Runtime tasks exposes Backtraces without a sampling field.",
    "Backtraces" => "Capture stacks for future activity. Does not reconstruct historical or missing stacks.",
    "Sampling" => "1/N accepts approximately one in N eligible events. Percentage = 100/N, not loss rate. Counts are observed records, not multiplied population estimates.",
    "Event buffer capacity" => "Events / thread. Larger rings cost more per-thread telemetry memory and retain more history. Capture important data before replacing buffers.",
    "Up/Down / Left/Right" => "Select field / change value.",
    "Enter" => "Apply the draft to the live process.",
    "Esc / F1" => "Cancel the dialog / open help without changing the draft or selected field.",
);

const FILTERS: &Section = section!("STACK FILTERS - WHOLE RECORDS";
    "Include" => "Comma/space-separated crate:name, module:crate::module, function:crate::module::name rules. Any captured frame may match; includes are ORed.",
    "Exclude" => "Same syntax; exclusions win. Empty Include and Exclude reset all records.",
    "Symbol ownership" => "Normalized symbol owners, not execution lineage, generic type arguments or implemented traits. module rules match descendants; function rules match the exact normalized path.",
    "Unknown stacks" => "Show/hide records where missing or unresolved frames prevent a definite decision.",
    "Runtime stack" => "Select event stack or spawn provenance. Spawn attribution does not imply execution at the spawn site.",
    "Events / Allocations / Tasks" => "Filter banner shown/total counts describe retained indexed populations. unknown counts incomplete attribution, not source loss.",
    "Background filtering" => "Current view remains available until rebuilding finishes. Help keeps its original topic even when completion resets focus.",
    "Unfiltered values" => "Source accepted/overwritten counters and native owner/backend inventory remain unfiltered.",
    "f versus F" => "Lowercase f only changes displayed stack frames; uppercase F filters whole records.",
    "Tab / Up/Down" => "Select a field.",
    "Typing / Backspace / Delete" => "Append Include/Exclude text / remove its last character.",
    "Left/Right / Space" => "Change options.",
    "Enter / Esc" => "Validate and apply / cancel. Invalid rules preserve the editable draft and existing view. Help never edits or applies drafts.",
);

const CAPTURE: &Section = section!("CAPTURE PROGRESS";
    "Capture process snapshot" => "Request process telemetry.",
    "Decode telemetry" => "Reconstruct analysis views.",
    "Save snapshot file" => "Persist the native snapshot.",
    "Phase N of 3 / gauge" => "Capture stage, not measured byte progress or a completion-time estimate.",
    "Elapsed" => "Seconds since capture began.",
    "Background completion" => "Capture continues while help is open. This topic stays fixed; close help to inspect results and capture/save failures.",
    "Record and continue" => "Default: capture and save a snapshot, preserving server buffers and the current recording policy. Enabled classes continue writing in the background; disabled classes stay disabled. Exited-thread buffers are released after capture.",
    "Record and stop" => "Capture events, stop all six recording classes and deallocate server event rings. Recorder metadata remains. Later source/decode/save errors do not undo a completed stop; live policy is read back even on errors.",
    "Clear (C)" => "Empty server buffers independently: no snapshot, symbolization or disk I/O. Keep active-thread allocations and prior recording policy. The displayed capture and saved files are unchanged.",
    "Snapshot scope" => "Capture is live-only. Analysis otherwise shows the last captured snapshot.",
);

const LOADING: &Section = section!("LOADING SNAPSHOT";
    "Background decoding" => "Read/decode the native file without connecting to a process. Empty panes while loading do not measure zero activity.",
    "Help context" => "Remains fixed if loading completes in the background.",
    "After loading" => "Same tabs, selections and filters as live captures; recording/capture controls remain disabled.",
    "Failure" => "File/decoding failures are reported as errors, not an empty successful snapshot.",
);

const ERROR: &Section = section!("SNAPSHOT ERRORS / UNAVAILABLE DATA";
    "Error message" => "Close help to read the underlying panel/status. Capture, decode, source and save failures are not measurements of zero activity.",
    "Last snapshot" => "A previous snapshot may remain visible after a failed refresh.",
    "Live recovery" => "Verify instrumented application/source, then press s to capture again.",
    "Offline recovery" => "Saved files are read-only. Recapture in the producing process if data was never recorded.",
    "Missing heap source" => "Does not necessarily invalidate other telemetry.",
    "No matching records" => "May be stack filters or missing backtraces; F changes filters.",
    "No runtime source/events" => "May mean an uninstrumented executor, not an idle one.",
);

const COMMON: &Section = section!("UNITS / SCOPE / MISSING DATA";
    "Counts" => "Recorded events or identities, not extrapolated totals.",
    "Bytes" => "Binary units: KiB = 1024 B, MiB = 1024 KiB, GiB = 1024 MiB.",
    "Durations" => "ns/us/ms/s = nanoseconds/microseconds/milliseconds/seconds.",
    "Retained window" => "Available records, not process lifetime.",
    "Accepted / overwritten" => "Source counters span event classes and exclude suppressed, sampled-out, disabled and non-producing activity.",
    "Loss percentage" => "Overwritten / accepted is ring loss, not sampling rate or percentage of all application work.",
    "Zero loss" => "Does not prove complete history.",
    "Zero / '-' / unavailable" => "Zero may mean no samples, disabled recording or unavailable counter coverage. '-' and unavailable messages explicitly mean missing data.",
    "Global versus filtered" => "Native owner/backend inventory remains unfiltered. Record totals can change with F filters; compare only matching scope and denominator.",
);

const HELP_CONTROLS: &Section = section!("HELP CONTROLS";
    "F1 / Esc / q" => "Close only help; preserve underlying dialog drafts and selections.",
    "Up/Down / mouse wheel" => "Scroll one visual line.",
    "PgUp/PgDn" => "Scroll one page.",
    "Home/End" => "Reach the beginning/end, including wrapped text.",
    "Resize" => "Rewrap text and clamp scroll position.",
    "Background work" => "Capture/filter completion does not switch this help topic.",
);

const LIVE: &Section = section!("LIVE NAVIGATION - AFTER CLOSING HELP";
    "1-8 / Tab / Shift-Tab" => "Select/cycle main tabs from every Runtime focus, including Activity.",
    "Click rows / drag borders" => "Select and enter a row's detail pane / resize panes.",
    "s" => "Run the selected Record and continue / Record and stop action. Analysis tables do not continuously refresh.",
    "c" => "Configure recording.",
    "d" => "Switch between Record and continue (default) and Record and stop.",
    "C (uppercase)" => "Clear server event buffers without capturing; preserve recording policy, displayed snapshot and saved files. Stop and Clear require a supporting server; legacy servers report an explicit unsupported error.",
    "F" => "Edit whole-record stack filters.",
    "Esc / q" => "Disconnect to browser / quit.",
    "A/E/X/R/I/C" => "Footer indicators: Allocations, general Events, Arc dereferences, Runtime tasks, I/O, Cache. Letter = enabled; '-' = disabled. Sampling/backtrace settings are in Info or c.",
);

const OFFLINE: &Section = section!("OFFLINE NAVIGATION - AFTER CLOSING HELP";
    "Read-only" => "Offline is read-only: no process connection, recording changes, capture, capture-mode changes or clearing server buffers.",
    "1-8 / Tab / Shift-Tab" => "Select/cycle tabs. Sorting, stack presentation and F filters change only the saved-data view.",
    "q / Esc" => "Quit the offline viewer. With help open these keys close help instead.",
    "Missing data" => "A live-only 'press s' placeholder cannot create missing data in an offline file.",
);

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXTS: [Context; 25] = [
        Context::Browser,
        Context::Info,
        Context::HeapBuckets,
        Context::HeapHotspots,
        Context::Allocations,
        Context::PrimitiveTypes,
        Context::PrimitiveOperations,
        Context::PrimitiveHotspots,
        Context::Threads,
        Context::ThreadOperations,
        Context::ThreadParticipants,
        Context::ThreadObjects,
        Context::RuntimeWorkers,
        Context::RuntimeTasks,
        Context::RuntimeActivity,
        Context::RuntimeEvents,
        Context::IoResources,
        Context::IoOperations,
        Context::CacheTiers,
        Context::CacheOperations,
        Context::Recording,
        Context::Filters,
        Context::Capture,
        Context::Loading,
        Context::Error,
    ];

    fn text(context: Context, offline: bool) -> String {
        document(context, offline)
            .iter()
            .flat_map(|section| {
                std::iter::once(section.heading).chain(section.entries.iter().flat_map(|entry| [entry.keyword, entry.explanation]))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn version_labels_are_explained_only_in_info_help() {
        for (context, offline) in [(Context::Info, false), (Context::Info, true)] {
            let help = text(context, offline);
            for label in [
                "Monitor (seismograph_cli)",
                "Server (seismograph)",
                "unknown (legacy server)",
                "unknown (not recorded in snapshot)",
            ] {
                assert!(help.contains(label), "{context:?}: missing {label}");
            }
        }
        for context in CONTEXTS {
            if context != Context::Info {
                assert!(!text(context, false).contains("CRATE VERSIONS"), "{context:?}");
                assert!(!text(context, true).contains("CRATE VERSIONS"), "{context:?}");
            }
        }
    }

    #[test]
    fn every_context_has_explicit_sections_entries_and_help_controls() {
        for context in CONTEXTS {
            for offline in [false, true] {
                assert!(!title(context).is_empty());
                for section in document(context, offline) {
                    assert!(!section.heading.is_empty());
                    assert!(!section.entries.is_empty());
                    for entry in section.entries {
                        assert!(!entry.keyword.is_empty());
                        assert!(!entry.explanation.is_empty());
                        assert!(entry.keyword.is_ascii() && entry.explanation.is_ascii());
                    }
                }
                let help = text(context, offline);
                for required in ["F1", "PgUp/PgDn", "Home/End"] {
                    assert!(help.contains(required), "{context:?}: missing {required}");
                }
                if context == Context::Info {
                    assert!(help.contains(if offline { "Offline is read-only" } else { "LIVE NAVIGATION" }));
                } else {
                    assert!(!help.contains(COMMON.heading));
                    assert!(!help.contains(LIVE.heading));
                    assert!(!help.contains(OFFLINE.heading));
                }
            }
        }
    }

    #[test]
    fn focused_help_excludes_focusable_siblings_and_keeps_passive_children() {
        for (context, sections) in [
            (Context::HeapBuckets, vec![HEAP_BUCKETS, HEAP_SUMMARY, HEAP_HOTSPOTS]),
            (Context::HeapHotspots, vec![HEAP_BUCKETS, HEAP_SUMMARY, HEAP_HOTSPOTS]),
            (Context::Allocations, vec![ALLOCATIONS, STACK]),
            (Context::PrimitiveTypes, vec![PRIMITIVE_TYPES]),
            (Context::PrimitiveOperations, vec![PRIMITIVE_OPERATIONS, PRIMITIVE_VALUES]),
            (Context::PrimitiveHotspots, vec![PRIMITIVE_HOTSPOTS, STACK]),
            (Context::Threads, vec![THREADS]),
            (Context::ThreadOperations, vec![THREAD_OPERATIONS, PRIMITIVE_VALUES]),
            (Context::ThreadParticipants, vec![THREAD_PARTICIPANTS]),
            (Context::ThreadObjects, vec![THREAD_OBJECTS, STACK]),
            (Context::RuntimeWorkers, vec![RUNTIME_WORKERS]),
            (Context::RuntimeTasks, vec![RUNTIME_TASKS, RUNTIME_STATES]),
            (Context::RuntimeActivity, vec![RUNTIME_ACTIVITY, RUNTIME_STATES]),
            (Context::RuntimeEvents, vec![RUNTIME_EVENTS]),
            (Context::IoResources, vec![IO_RESOURCES]),
            (Context::IoOperations, vec![IO_OPERATIONS]),
            (Context::CacheTiers, vec![CACHE_TIERS]),
            (Context::CacheOperations, vec![CACHE_OPERATIONS]),
        ] {
            let mut expected = sections;
            expected.push(HELP_CONTROLS);
            for offline in [false, true] {
                assert_eq!(document(context, offline), expected, "{context:?} offline={offline}");
            }
        }
    }

    const TABLE_COLUMNS: &[(Context, &[&str])] = &[
        (
            Context::Info,
            &[
                "Name",
                "Instance",
                "PID",
                "Monitor port",
                "Event ring buffer",
                "Telemetry memory / thread",
                "Telemetry threads",
                "Telemetry memory total",
                "Accepted telemetry events",
                "Retained telemetry events",
                "Overwritten telemetry events",
                "events/s",
                "total",
            ],
        ),
        (
            Context::HeapBuckets,
            &[
                "Endpoint / lease",
                "Freshness",
                "Last contributor",
                "Object / slab / capacity",
                "Large outstanding",
                "Local backend / metadata",
                "Reservations",
                "Owners",
                "Publication",
            ],
        ),
        (
            Context::HeapHotspots,
            &[
                "Outgoing returns",
                "Batching budget",
                "Incoming atomic queue",
                "Capture consistency",
            ],
        ),
        (
            Context::Allocations,
            &["Allocations", "Allocated", "Average", "Unmatched", "Unmatched B", "Location"],
        ),
        (Context::PrimitiveTypes, &["Type", "Events", "Objects", "Contentions"]),
        (
            Context::PrimitiveOperations,
            &["Operation", "Events", "Objects", "Threads", "Hotspots"],
        ),
        (Context::PrimitiveHotspots, &["Events", "Location", "Stack Trace"]),
        (Context::Threads, &["Thread", "Retained", "Overwritten"]),
        (Context::ThreadOperations, &["Operation", "Events", "Objects", "Threads"]),
        (Context::ThreadParticipants, &["Thread", "Objects", "Events"]),
        (Context::ThreadObjects, &["Object", "Own", "Related", "event(s)"]),
        (
            Context::RuntimeWorkers,
            &[
                "Runtime / thread",
                "Role",
                "State",
                "Tasks",
                "Polls",
                "Observed %",
                "Median poll",
                "Max poll",
                "Poll timeline",
            ],
        ),
        (
            Context::RuntimeTasks,
            &["Task", "State(global)", "Polls", "Observed %", "Median poll", "Max poll"],
        ),
        (
            Context::RuntimeActivity,
            &[
                "Running for / Ready for",
                "Poll (worker)",
                "Ready (global)",
                "Duration (log10 buckets) / count",
                "Completed only",
            ],
        ),
        (
            Context::IoResources,
            &["Resource", "Kind", "Reads", "Writes", "Requested", "Completed", "Errors"],
        ),
        (
            Context::IoOperations,
            &["Operation", "Type", "Outcome", "Thread", "Requested", "Completed", "Duration"],
        ),
        (
            Context::CacheTiers,
            &["Tier identity", "Role", "Events", "Hits", "Misses", "Errors", "Hit rate"],
        ),
        (Context::CacheOperations, &["Outcome", "Events", "Refresh suppressed"]),
    ];
    #[test]
    fn rendered_table_columns_have_their_own_labeled_entries() {
        for (context, columns) in TABLE_COLUMNS {
            let document = document(*context, false);
            for column in *columns {
                assert!(
                    document
                        .iter()
                        .flat_map(|section| section.entries)
                        .any(|entry| entry.keyword == *column),
                    "{context:?}: missing separate column entry {column}"
                );
            }
        }
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one table verifies each focused panel's metric semantics")]
    fn help_explains_noninterchangeable_scopes_and_operation_values() {
        for (context, distinctions) in [
            (
                Context::RuntimeTasks,
                &[
                    "lifetime",
                    "THIS selected worker",
                    "not CPU utilization",
                    "no samples",
                    "Spawned",
                    "Materialized",
                    "Waiting",
                    "not proof of Poll::Pending",
                    "terminal retirement",
                    "Completed",
                    "Canceled",
                    "Panicked",
                ][..],
            ),
            (
                Context::RuntimeActivity,
                &["poll finish", "raw wake", "queue waiting", "12", "vertically", "Dim baseline"][..],
            ),
            (
                Context::RuntimeWorkers,
                &[
                    "displayed per-runtime window",
                    "NOT CPU",
                    "unassigned",
                    "Core",
                    "Blocking",
                    "Io",
                    "Running",
                    "Parked",
                    "Stopped",
                    "not necessarily polling now",
                ][..],
            ),
            (
                Context::Allocations,
                &["process-live", "zero overwrites", "without a matched", "Sampling"][..],
            ),
            (
                Context::HeapBuckets,
                &[
                    "not app-live memory",
                    "not the current lease holder",
                    "unknown, not zero",
                    "NOT pending or queued bytes",
                ][..],
            ),
            (
                Context::PrimitiveTypes,
                &[
                    "Channel receive wait",
                    "not nanoseconds",
                    "Arc",
                    "Mutex",
                    "RwLock",
                    "Barrier",
                    "Condvar",
                    "OnceLock / LazyLock",
                ][..],
            ),
            (
                Context::PrimitiveOperations,
                &[
                    "Create",
                    "Clone",
                    "Deref",
                    "Final drop",
                    "Relocate",
                    "Acquisition",
                    "Release",
                    "Read acquisition",
                    "Write contention",
                    "Completed wait",
                    "Blocked wait",
                    "Generation release",
                    "Notification",
                    "Initialization",
                    "Send contention",
                    "Receive wait (empty)",
                    "High watermark",
                    "Poison observed",
                    "Poison cleared",
                ][..],
            ),
            (
                Context::CacheTiers,
                &["Hits / (Hits + Misses + Errors)", "non-lookup operations"][..],
            ),
            (Context::IoOperations, &["pair is incomplete", "not zero latency"][..]),
            (Context::Recording, &["enabled + backtraces on + sampling 1 (100%)"][..]),
            (
                Context::Filters,
                &["includes are ORed", "exclusions win", "spawn provenance", "not execution lineage"][..],
            ),
        ] {
            let help = text(context, false);
            for distinction in distinctions {
                assert!(help.contains(distinction), "{context:?}: missing {distinction}");
            }
        }
    }

    #[test]
    fn offline_info_does_not_claim_live_statistics_or_capture_time() {
        let help = text(Context::Info, true);
        assert!(help.contains("Capture time is not recorded"));
        assert!(help.contains("There is no live activity polling"));
        assert!(!help.contains("LIVE ACTIVITY"));
    }

    #[test]
    fn rule_syntax_and_colons_are_explanation_content_not_structure() {
        let includes = FILTERS.entries.iter().find(|entry| entry.keyword == "Include").unwrap();
        assert!(
            includes
                .explanation
                .contains("crate:name, module:crate::module, function:crate::module::name")
        );
        assert_eq!(FILTERS.entries.iter().filter(|entry| entry.keyword == "Include").count(), 1);
    }
}

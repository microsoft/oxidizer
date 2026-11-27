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
const NATIVE_KEYS: &Section = section!("Navigation";
    "Enter" => "Focus the selected subsystem, class list, class detail or memory category detail. Cyan borders identify keyboard focus; Escape from memory detail returns to the category selector.",
    "Escape / Backspace" => "Move up one level. Escape disconnects or leaves an offline capture only at the allocator root.",
    "Up/Down / Home/End" => "Select list rows; in focused details, scroll text. Lists retain global indices when longer than the viewport.",
    "PgUp / PgDn" => "Move five rows in lists or scroll focused details by five rendered lines. Mouse rows select and focus their list; Enter opens the selection.",
    "b" => "If both allocator sources are present, switch between snmalloc and mimalloc. Each backend keeps its own selection.",
);
const MIMALLOC_GLOBAL: &Section = section!("Mimalloc global state";
    "Reservation" => "Arena reservation plus direct OS reservation includes metadata and padding. Arena metadata is already included. Do not add it again.",
    "Commitment" => "Arena bitmap commitment includes tracked arena metadata. Direct OS data commitment excludes metadata commitment. Total OS commitment is unknown.",
    "Residency" => "Bitmap commit is not physical residency. Resident and swapped memory are unknown.",
    "Free / Pending purge slices" => "Free bits advertise reusable arena slices. Pending purge bits await an attempt and do not guarantee that slices can be purged.",
    "INDEXED regular abandoned pages" => "The index covers regular bins 0..60 only, not live objects. Huge/singleton pages and some OS-backed pages are unindexed. Zeros do not prove their absence. Owner queues exclude abandoned pages.",
    "Independent observations" => "Global scans read independent atomics. Arena scans are bounded. Partial scans omit arenas and are not a simultaneous census.",
);
const MIMALLOC_OWNERS: &Section = section!("Mimalloc heap lifetimes";
    "Owner" => "A stable heap lifetime identifier, not a pointer or a recorder thread. Selection follows this identifier across captures.",
    "Active / Exited" => "Active heaps can publish state. Exited heaps retain historical copies after cleanup. Heap identifiers are never reused.",
    "Freshness" => "Matching session and round means contributed this round, not an exact current census. Age is measured at capture.",
    "Busy / Unobserved / Unavailable" => "A busy publication slot, no readable copy, or diagnostic storage allocation failure. Missing measurements are unknown, not zero.",
    "Publication" => "Completed allocation, resize, and free operations can publish while allocation recording is enabled. Event sampling does not control publication. A live capture requests the next round after collection.",
    "Filters" => "Event filters preserve the original allocator inventory, observations and completeness flags.",
);
const MIMALLOC_CLASSES: &Section = section!("Mimalloc page classes";
    "Reserved / Capacity blocks" => "Both counts describe visited pages only. Reserved slots include uninitialized blocks. Capacity counts initialized slots. Neither count measures application-live objects.",
    "Used blocks" => "Owner-accounted used blocks include pending remote frees until the owner collects them.",
    "Free / Local free blocks" => "Visited allocation-ready links and locally deferred links. Free-list and page walks have separate visit budgets.",
    "Queued / Observed pages" => "Queued pages include omitted queue entries but exclude abandoned pages. Observed pages include only visited entries.",
    "Retired pages" => "Visited pages with an active retirement countdown.",
    "Partial" => "A bounded page or free-list walk ended early. Shown values describe visited state, not the omitted entries.",
    "Zero geometry" => "A zero block or page size means no fixed geometry for that bin. Singleton pages can have variable sizes.",
);
const NATIVE_MEMORY: &Section = section!("Memory measurements";
    "OS reserved" => "Native allocator virtual reservations: cumulative successful OS object/range reservations plus the separate sparse page-map VA reservation. The sum uses 128-bit arithmetic. This is not physical RAM or app-live memory.",
    "Allocator ranges" => "Cumulative successful native OS reservations, including native metadata backing. They exclude the sparse page map and are not a current live-allocation census.",
    "Page-map VA" => "Sparse virtual address reservation, normally 256 GiB. Virtual address space is not committed or resident physical memory.",
    "Committed / Resident / Swapped" => "Not recorded by this source. Unknown (dash) is not zero. No process polling or inferred physical-memory ratios are used.",
    "Breakdown" => "The reserved breakdown contains only the two recorded reservation counters. Independently captured owner capacities must not be summed into an accurate global pie or subtracted to invent allocated/unaccounted memory.",
);
const NATIVE_GLOBAL: &Section = section!("Global backend";
    "Shared backend" => "Shared range supply beneath owner frontends. The chart shows observed range counts by power-of-two size.",
    "Cached capacity" => "Observed cached range capacity, not guaranteed physically decommitted memory. Partial walks report a lower observed count, not complete occupancy.",
    "Local boundary / Refill ceiling" => "Range-cache policy thresholds, not amounts of app-live memory or pending bytes.",
);
const NATIVE_OWNERS: &Section = section!("Owner frontends";
    "Owner" => "A reusable native allocator endpoint with small-class slabs, large ranges, local caches and remote returns. Owner numbers follow the current inventory order and can change on refresh; selection follows stable endpoint identity. Endpoint identity appears only in Observation detail.",
    "Active / Idle" => "Active means leased. Idle endpoints can be freshly inspected under the pool lock; active endpoints are read from immutable published summaries, never by inspecting their borrowed core.",
    "Contributor" => "The recorder thread that last published the observation, not necessarily the current lease holder. Idle inspection has no recorder contributor.",
    "Quiet owners" => "Publication follows completed operations while allocation recording is enabled, independently of event sampling. Never-contributing active endpoints remain explicitly Unknown.",
);
const NATIVE_COVERAGE: &Section = section!("Observation coverage";
    "Observed" => "Published in this session, lease generation and requested observation round, or freshly inspected while idle. Matching round means contributed this round, not an exact simultaneous census.",
    "Older" => "Stored evidence from an older session, lease generation or round. It describes the earlier observation, not an unobserved current lease.",
    "Unknown / Busy / Unavailable" => "No observation / publication slot busy / System slot allocation failed. Missing evidence is never displayed as zero.",
    "Complete" => "Inventory or structure walk finished its bounded collection. This describes collection coverage, not observation freshness; a complete owner inventory can contain Older or Unknown endpoints.",
    "Partial" => "The bounded owner inventory or structure walk did not finish. Shown capacities and counts include only observed entries.",
    "Inventory statistics" => "Active/Idle and evidence counts describe captured owner rows, not omitted endpoints. Evidence categories count each row once; Busy or Unavailable takes priority over stored freshness. Busy rows can retain older evidence.",
    "Independent observations" => "Owners may have different observation times and transfer ranges between them. Their capacities can overlap across time; do not add them into app-live totals.",
    "Polling" => "Capture requests the next observation round after collection. No timers or background publication threads are added; quiet owners can retain older evidence.",
);
const NATIVE_CLASSES: &Section = section!("Small classes";
    "Object / Slab / Slots" => "Nominal allocation-class size and slab geometry. Capacity is class-rounded allocator storage, not requested application bytes or live object count.",
    "Available / Empty / Fast" => "Observed slab-list membership and fast-list state. These describe allocator structures, not who allocated or freed objects; use Allocations for event correlation and stacks.",
    "Partial" => "Slab walks are bounded. Counts from incomplete walks are observed entries only.",
);
const NATIVE_RANGES: &Section = section!("Large ranges";
    "Range capacity" => "Outstanding native allocator ranges include retained and pending frees. Range counts are not application-live allocation counts.",
    "Bars" => "Relative observed range counts within this selected structure. They are not physical memory percentages.",
);
const NATIVE_CACHES: &Section = section!("Local caches";
    "Local ranges" => "Owner-local backend range cache, grouped by power-of-two capacity.",
    "Metadata cache" => "Observed native metadata backing cache. Other metadata pools are not exhaustively inventoried.",
    "Capacity" => "Allocator capacity only; physical residency and decommit state are not recorded.",
);
const NATIVE_RETURNS: &Section = section!("Remote returns";
    "Flow" => "Cross-owner frees enter open rings, outgoing message buckets, and the destination owner's incoming queue.",
    "Message capacity" => "Class-rounded storage represented by observed messages, not requested bytes or a precise pending-live-memory count.",
    "Batch budget" => "Allocator remote-return batching budget, not pending bytes and not telemetry publication policy.",
    "Inbox work" => "Different front/back pointers indicate potential work, not queue depth or guaranteed ready links. Equal pointers do not establish emptiness.",
);
pub(super) const fn title(context: Context) -> &'static str {
    match context {
        Context::Browser => "Applications",
        Context::Info => "Info and activity",
        Context::NativeMemory => "Native allocator memory",
        Context::NativeGlobal => "Shared global backend",
        Context::NativeOwner => "Native owner frontends",
        Context::NativeClasses => "Small classes and slab geometry",
        Context::NativeRanges => "Large allocator ranges",
        Context::NativeCaches => "Owner-local range caches",
        Context::NativeReturns => "Remote returns",
        Context::NativeObservation => "Observation metadata",
        Context::MimallocGlobal => "Mimalloc global backend",
        Context::MimallocOwner => "Mimalloc heap lifetimes",
        Context::MimallocClasses => "Mimalloc page classes",
        Context::Allocations => "Allocation hotspots and stack",
        Context::PrimitiveTypes => "Primitive types",
        Context::PrimitiveOperations => "Primitive operations",
        Context::PrimitiveHotspots => "Primitive hotspots and stack",
        Context::Threads => "Threads",
        Context::ThreadOperations => "Thread operations",
        Context::ThreadParticipants => "Related threads",
        Context::ThreadObjects => "Thread objects and stacks",
        Context::RuntimeWorkers => "Runtime workers",
        Context::RuntimeTask => "Runtime task detail",
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
        Context::NativeMemory => &[NATIVE_MEMORY, NATIVE_COVERAGE, NATIVE_KEYS],
        Context::NativeGlobal => &[NATIVE_GLOBAL, NATIVE_MEMORY, NATIVE_KEYS],
        Context::NativeOwner | Context::NativeObservation => &[NATIVE_OWNERS, NATIVE_COVERAGE, NATIVE_KEYS],
        Context::NativeClasses => &[NATIVE_CLASSES, NATIVE_COVERAGE, NATIVE_KEYS],
        Context::NativeRanges => &[NATIVE_RANGES, NATIVE_COVERAGE, NATIVE_KEYS],
        Context::NativeCaches => &[NATIVE_CACHES, NATIVE_COVERAGE, NATIVE_KEYS],
        Context::NativeReturns => &[NATIVE_RETURNS, NATIVE_COVERAGE, NATIVE_KEYS],
        Context::MimallocGlobal => &[MIMALLOC_GLOBAL, NATIVE_KEYS],
        Context::MimallocOwner => &[MIMALLOC_OWNERS, NATIVE_KEYS],
        Context::MimallocClasses => &[MIMALLOC_CLASSES, MIMALLOC_OWNERS, NATIVE_KEYS],
        Context::Allocations => &[ALLOCATIONS, STACK],
        Context::PrimitiveTypes => &[PRIMITIVE_TYPES],
        Context::PrimitiveOperations => &[PRIMITIVE_OPERATIONS],
        Context::PrimitiveHotspots => &[PRIMITIVE_HOTSPOTS, STACK],
        Context::Threads => &[THREADS],
        Context::ThreadOperations => &[THREAD_OPERATIONS],
        Context::ThreadParticipants => &[THREAD_PARTICIPANTS],
        Context::ThreadObjects => &[THREAD_OBJECTS, STACK],
        Context::RuntimeWorkers => &[RUNTIME_WORKERS, RUNTIME_TASK_GROUPS],
        Context::RuntimeTask => &[RUNTIME_TASK_GROUPS, TASK_EVENTS],
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
    if matches!(context, Context::Info) {
        document.extend([VERSIONS, COMMON, if offline { OFFLINE } else { LIVE }]);
    }
    document.push(HELP_CONTROLS);
    document
}

const VERSIONS: &Section = section!("CRATE VERSIONS";
    "Monitor (seismograph_cli)" => "Semantic version compiled into this local monitor executable. Shown in the Info view for live connections and saved snapshots.",
    "Server (seismograph)" => "Semantic version of the seismograph crate in the connected application, queried during discovery after the OS-authorized handshake, before any capture. Shown in the Info view. This is not the protocol crate version or the application's own version.",
    "unknown (not recorded in snapshot)" => "Saved snapshots do not contain the server crate version. The local monitor version is not the snapshot producer's version.",
);

const BROWSER: &Section = section!("APPLICATIONS";
    "Application / instance" => "Discovered application name, with an optional instance name in parentheses.",
    "pid" => "Operating-system process ID of the application.",
    "owner" => "Owning Unix UID or Windows user SID. Registries are created automatically. Regular users can connect to their own processes; root/elevated administrators can discover and connect across accessible user registries.",
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
    "OS owner" => "Verified owning Unix UID or Windows user SID. Access is enforced by the operating system, not an authentication token.",
    "Endpoint" => "Local Unix socket path or Windows named-pipe name. Knowing the name does not grant connection access.",
    "Event ring buffer" => "Capacity in events per recording thread.",
    "Allocations / General events / Arc dereferences / Runtime tasks / I/O / Cache" => "Independent recording policies. Each shows enabled state, backtraces on/off, and sample 1/N (percentage = 100/N).",
    "Telemetry memory / thread" => "Ring-buffer memory for the configured capacity, not application heap usage.",
    "Active recorder threads" => "Current recorder-thread count from live server statistics. It excludes retired source threads retained in a snapshot.",
    "Live telemetry memory total" => "Current allocated recorder storage from live server statistics.",
    "Captured source threads" => "Recorder threads retained in the current snapshot, including retired sources.",
    "Estimated buffer memory" => "Configured memory per thread multiplied by captured source-thread count. This estimate is not live allocated memory.",
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

const ALLOCATIONS: &Section = section!("ALLOCATION HOTSPOTS";
    "Allocations" => "Retained allocation-event count for this captured stack.",
    "Allocated" => "Sum of requested allocation bytes.",
    "Average" => "Allocated / Allocations in integer bytes; zero when the count is zero.",
    "Maximum" => "Largest retained requested allocation size for this captured stack.",
    "Unmatched" => "Retained allocation count without a matched retained free.",
    "Unmatched B" => "Corresponding unmatched bytes. Neither unmatched metric proves process-live allocations or leaks, even with zero overwrites.",
    "Location" => "First displayed frame. Stack Trace shows this group's captured call stack.",
    "All-class source accepted / overwritten" => "Source counts spanning event classes, not allocation populations.",
    "Sampling / recording boundaries" => "Can omit a matching free and break live-allocation inference.",
    "Up/Down" => "Select a hotspot.",
    "Column headers / [ / ] / r" => "Click a metric header or use the keys to change the sort column and direction.",
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
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
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
    "Column headers / [ / ] / r" => "Click Events, Objects, Threads or Hotspots, or use the keys, to change sorting.",
);

const PRIMITIVE_HOTSPOTS: &Section = section!("PRIMITIVE HOTSPOTS";
    "Events" => "Retained records sharing this stack for the selected type and operation.",
    "Location" => "First displayed frame of the selected hotspot.",
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
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
    "Thread" => "Recorder thread ID and available name, not necessarily OS thread ID. The default order shows the most retained activity first.",
    "Retained" => "Retained source records for this thread; filtering can reduce this count.",
    "Overwritten" => "All-class source lost-record count for this thread; remains unfiltered.",
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
    "Up/Down / Enter" => "Select a thread / enter its Operations.",
    "Empty operations" => "Other event classes can contribute retained events without relevant operation rows.",
);

const THREAD_OPERATIONS: &Section = section!("THREAD OPERATIONS";
    "Operation" => "Selected thread's recorded operation category.",
    "Events" => "Its retained records in that category.",
    "Objects" => "Distinct associated recorded identities.",
    "Threads" => "Related participant rows, not all process threads.",
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
    "Relationships" => "Association through retained object identities, not a scheduler wait-for graph or proof one thread blocked another. Allocation/free operations connect their counterparts.",
    "Channel receive wait" => "Often normal producer/consumer behavior, not a bug.",
    "Up/Down / Enter / Backspace" => "Select operation / enter Related Threads / return to Threads.",
);

const THREAD_PARTICIPANTS: &Section = section!("RELATED THREADS";
    "Thread" => "Related recorder thread; (self) is the originally selected thread.",
    "Objects" => "Shared recorded objects in this relationship.",
    "Events" => "Participant's retained events on those objects.",
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
    "Relationship title" => "Allocation/free counterparts or primitive users, including self. Shared identity does not establish causality, wall-clock overlap or lock ownership.",
    "Up/Down / Enter / Backspace" => "Select participant / enter Objects / return to Operations.",
);

const THREAD_OBJECTS: &Section = section!("THREAD OBJECTS / STACKS";
    "Object" => "Hexadecimal recorded identity, not a size or portable pointer.",
    "Own" => "Selected thread's retained events for this operation/object.",
    "Related" => "Participant's corresponding retained events.",
    "Total" => "Own + Related. The default order ranks objects by this value, not elapsed time.",
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
    "Selected thread operation / Related thread operation" => "Representative captured stack on each side, not a complete chronological trace.",
    "event(s)" => "Retained count for that representative stack, not the entire object's event count.",
    "Missing stacks" => "Backtrace settings and partial retention can remove attribution on either side.",
    "Up/Down / Backspace" => "Select object / go up.",
    "f / PgUp/PgDn" => "Toggle application/all frames / scroll both stack sections.",
);

const RUNTIME_WORKERS: &Section = section!("RUNTIME WORKERS";
    "Runtime" => "Selected runtime name and ID. Use [ and ] when the snapshot contains more than one runtime.",
    "Worker ID" => "Runtime worker ID. The selected runtime supplies the scope for this value.",
    "State" => "Running means available to execute, not necessarily polling now. Parked means parked; Stopped means stopped. Event-only workers can have blank metadata.",
    "Types" => "Distinct retained task descriptors that executed on this worker.",
    "Tasks" => "Distinct asynchronous tasks with a retained poll on this worker. A migrated task can count on more than one worker.",
    "Runs" => "Blocking executions with a retained start on this worker.",
    "Succeeded / Canceled / Panicked" => "Retained terminal outcomes for blocking tasks that started on this worker.",
    "Polls" => "Retained completed polls on this worker, not a lifetime count. An ongoing poll contributes to the timeline, not this count.",
    "Median poll" => "Median retained completed poll duration across this worker's tasks; '-' without samples and 0ns for measured zero.",
    "Max poll" => "Longest retained completed poll duration on this worker; an ongoing poll is not a completed sample.",
    "Poll timeline" => "Each async worker row bins retained poll intervals on the runtime window. A dim baseline means unobserved time, not measured idle time.",
    "Sortable headers" => "Click a heading to sort by that column. Poll timeline sorts by total retained poll interval time. Click a heading again to reverse the order.",
    "Avg queue / Max queue" => "Mean and maximum retained time that blocking tasks waited before this worker started them.",
    "Avg exec / Max exec" => "Mean and maximum retained blocking execution duration on this worker.",
    "Worker Activity" => "The panel beside the worker list shows the selected worker's category-specific totals and durations.",
    "No runtime data" => "A recorder does not instrument executors automatically. Runtime source presence alone is not event history.",
    "a / b / Up / Down / Enter" => "Select async or blocking workers, move the worker selection, or open the matching task-type page.",
);

const RUNTIME_TASK_GROUPS: &Section = section!("RUNTIME TASK TYPES";
    "Spawned at" => "Source file, line and column for the task descriptor. A numeric descriptor appears when the source location is unavailable.",
    "Future B" => "Inline future size shared by this asynchronous task type.",
    "Closure B" => "Inline closure size shared by this blocking task type.",
    "Async tasks" => "Groups are inferred from retained poll events. Poll totals include retained finished and unfinished polls.",
    "Blocking tasks" => "Groups are inferred when a blocking worker starts execution. Queue-only tasks remain unclassified.",
    "Spawned" => "Retained task count for this descriptor.",
    "Live" => "Tasks in queued, running, or waiting state at the end of the capture.",
    "Succeeded" => "Tasks with a retained successful completion.",
    "Canceled" => "Tasks with a retained cancellation.",
    "Panicked" => "Tasks with a retained panic outcome.",
    "Workers" => "Distinct workers that executed a retained poll or blocking execution.",
    "Polls" => "Retained async polls. An unfinished poll is included but has no duration sample.",
    "Runs" => "Blocking executions with a retained start event. A canceled queue entry has no run.",
    "Avg ready / Max ready" => "Mean and maximum retained queue wait before an async poll. Raw wake latency is excluded.",
    "Avg poll" => "Mean duration from retained finished async polls.",
    "Max poll" => "Maximum duration from retained finished async polls.",
    "Avg queue" => "Mean retained blocking queue duration. The duration ends when a blocking worker starts the task.",
    "Max queue" => "Maximum retained blocking queue duration.",
    "Avg exec" => "Mean retained blocking execution duration.",
    "Max exec" => "Maximum retained blocking execution duration. An unfinished execution has no duration sample.",
    "a / b / Up / Down" => "Select the async or blocking task-type page, then move the descriptor selection.",
);

const TASK_EVENTS: &Section = section!("RUNTIME TASK EVENTS";
    "Operations" => "Inferred operation categories attributed to the selected task.",
    "Events" => "Retained occurrences attributed to that operation and task.",
    "Time (ns)" => "Process-monotonic event timestamp in nanoseconds. It is not wall-clock time.",
    "Thread" => "Recorder thread that emitted the retained event.",
    "Sequence" => "Per-thread retained event sequence.",
    "Object" => "Recorded object identity associated with the event.",
    "unassigned / ambiguous" => "Retained events that could not be attributed to one task, or matched more than one task. They are excluded from task operation counts.",
    "Sortable headers" => "Click a heading to sort by that column. Click it again to reverse the order.",
    "e / Up / Down / Enter / Backspace" => "Focus event analysis, move the selection, open occurrences, or return to operations.",
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
    "Interval" => "Retained finish timestamp minus retained start timestamp, in ns/us/ms/s. This interval includes start-event recording overhead, including backtrace capture. It is not pure device latency. '-' means the pair is incomplete, not zero latency.",
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
    "1-9 / Tab / Shift-Tab" => "Select or cycle main tabs from every Runtime focus, including Activity.",
    "Click rows / drag borders" => "Select and enter a row's detail pane / resize panes.",
    "s" => "Run the selected Record and continue / Record and stop action. Analysis tables do not continuously refresh.",
    "c" => "Configure recording.",
    "d" => "Switch between Record and continue (default) and Record and stop.",
    "C (uppercase)" => "Clear server event buffers without capturing; preserve recording policy, displayed snapshot and saved files. A disconnected server produces an error; commands are never replaced with a different action.",
    "F" => "Edit whole-record stack filters.",
    "Esc / q" => "Disconnect to browser / quit.",
    "A/E/X/R/I/C" => "Footer indicators: Allocations, general Events, Arc dereferences, Runtime tasks, I/O, Cache. Letter = enabled; '-' = disabled. Sampling/backtrace settings are in Info or c.",
);

const OFFLINE: &Section = section!("OFFLINE NAVIGATION - AFTER CLOSING HELP";
    "Read-only" => "Offline is read-only: no process connection, recording changes, capture, capture-mode changes or clearing server buffers.",
    "1-9 / Tab / Shift-Tab" => "Select or cycle tabs. Sorting, stack presentation, and F filters change only the saved-data view.",
    "q / Esc" => "Quit the offline viewer. With help open these keys close help instead.",
    "Missing data" => "A live-only 'press s' placeholder cannot create missing data in an offline file.",
);

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXTS: [Context; 32] = [
        Context::Browser,
        Context::Info,
        Context::NativeMemory,
        Context::NativeGlobal,
        Context::NativeOwner,
        Context::NativeClasses,
        Context::NativeRanges,
        Context::NativeCaches,
        Context::NativeReturns,
        Context::NativeObservation,
        Context::MimallocGlobal,
        Context::MimallocOwner,
        Context::MimallocClasses,
        Context::Allocations,
        Context::PrimitiveTypes,
        Context::PrimitiveOperations,
        Context::PrimitiveHotspots,
        Context::Threads,
        Context::ThreadOperations,
        Context::ThreadParticipants,
        Context::ThreadObjects,
        Context::RuntimeWorkers,
        Context::RuntimeTask,
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
            (Context::NativeOwner, vec![NATIVE_OWNERS, NATIVE_COVERAGE, NATIVE_KEYS]),
            (Context::NativeReturns, vec![NATIVE_RETURNS, NATIVE_COVERAGE, NATIVE_KEYS]),
            (Context::Allocations, vec![ALLOCATIONS, STACK]),
            (Context::PrimitiveTypes, vec![PRIMITIVE_TYPES]),
            (Context::PrimitiveOperations, vec![PRIMITIVE_OPERATIONS, PRIMITIVE_VALUES]),
            (Context::PrimitiveHotspots, vec![PRIMITIVE_HOTSPOTS, STACK]),
            (Context::Threads, vec![THREADS]),
            (Context::ThreadOperations, vec![THREAD_OPERATIONS, PRIMITIVE_VALUES]),
            (Context::ThreadParticipants, vec![THREAD_PARTICIPANTS]),
            (Context::ThreadObjects, vec![THREAD_OBJECTS, STACK]),
            (Context::RuntimeWorkers, vec![RUNTIME_WORKERS, RUNTIME_TASK_GROUPS]),
            (Context::RuntimeTask, vec![RUNTIME_TASK_GROUPS, TASK_EVENTS]),
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
                "OS owner",
                "Endpoint",
                "Event ring buffer",
                "Telemetry memory / thread",
                "Active recorder threads",
                "Live telemetry memory total",
                "Captured source threads",
                "Estimated buffer memory",
                "Accepted telemetry events",
                "Retained telemetry events",
                "Overwritten telemetry events",
                "events/s",
                "total",
            ],
        ),
        (
            Context::NativeOwner,
            &[
                "Owner",
                "Active / Idle",
                "Contributor",
                "Quiet owners",
                "Observed",
                "Older",
                "Partial",
            ],
        ),
        (
            Context::NativeReturns,
            &["Flow", "Batch budget", "Inbox work", "Independent observations"],
        ),
        (
            Context::Allocations,
            &[
                "Allocations",
                "Allocated",
                "Average",
                "Maximum",
                "Unmatched",
                "Unmatched B",
                "Location",
            ],
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
                "Worker ID",
                "State",
                "Types",
                "Tasks",
                "Runs",
                "Succeeded / Canceled / Panicked",
                "Polls",
                "Median poll",
                "Max poll",
                "Poll timeline",
                "Avg queue / Max queue",
                "Avg exec / Max exec",
            ],
        ),
        (
            Context::RuntimeTask,
            &[
                "Spawned at",
                "Spawned",
                "Live",
                "Succeeded",
                "Canceled",
                "Panicked",
                "Workers",
                "Polls",
                "Runs",
                "Avg poll",
                "Max poll",
                "Avg queue",
                "Max queue",
                "Avg exec",
                "Max exec",
            ],
        ),
        (
            Context::IoResources,
            &["Resource", "Kind", "Reads", "Writes", "Requested", "Completed", "Errors"],
        ),
        (
            Context::IoOperations,
            &["Operation", "Type", "Outcome", "Thread", "Requested", "Completed", "Interval"],
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
            (Context::RuntimeTask, &["Canceled", "Panicked", "queue", "execution"][..]),
            (
                Context::RuntimeWorkers,
                &["Blocking", "Running", "Parked", "Stopped", "not necessarily polling now"][..],
            ),
            (
                Context::Allocations,
                &["process-live", "zero overwrites", "without a matched", "Sampling"][..],
            ),
            (
                Context::NativeReturns,
                &[
                    "not pending bytes",
                    "not queue depth",
                    "never displayed as zero",
                    "not an exact simultaneous census",
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

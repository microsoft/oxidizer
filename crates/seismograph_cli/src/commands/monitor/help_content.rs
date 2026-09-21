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
        Context::HeapBuckets => "Heaps: summary, tiers and buckets",
        Context::HeapHotspots => "Heaps: allocation locations and stack",
        Context::Allocations => "Allocation hotspots and stack",
        Context::PrimitiveTypes => "Primitive types",
        Context::PrimitiveOperations => "Primitive operations",
        Context::PrimitiveHotspots => "Primitive hotspots and stack",
        Context::Threads => "Threads",
        Context::ThreadOperations => "Thread operations",
        Context::ThreadParticipants => "Related threads",
        Context::ThreadObjects => "Thread objects and stacks",
        Context::RuntimeWorkers => "Runtime threads",
        Context::RuntimeTasks => "Runtime tasks",
        Context::RuntimeDetails => "Runtime task details",
        Context::RuntimeSpawnStack => "Runtime spawn stack",
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
                &[OFFLINE_INFO]
            } else {
                &[INFO, ACTIVITY]
            }
        }
        Context::HeapBuckets => &[HEAP_BUCKETS, HEAP_SUMMARY, HEAP_HOTSPOTS, STACK],
        Context::HeapHotspots => &[HEAP_HOTSPOTS, STACK, HEAP_BUCKETS, HEAP_SUMMARY],
        Context::Allocations => &[ALLOCATIONS, STACK],
        Context::PrimitiveTypes => &[PRIMITIVE_TYPES, PRIMITIVE_OPERATIONS, PRIMITIVE_HOTSPOTS, STACK],
        Context::PrimitiveOperations => &[PRIMITIVE_OPERATIONS, PRIMITIVE_HOTSPOTS, STACK, PRIMITIVE_TYPES],
        Context::PrimitiveHotspots => &[PRIMITIVE_HOTSPOTS, STACK, PRIMITIVE_OPERATIONS, PRIMITIVE_TYPES],
        Context::Threads => &[THREADS, THREAD_OPERATIONS, THREAD_PARTICIPANTS, THREAD_OBJECTS, STACK],
        Context::ThreadOperations => &[THREAD_OPERATIONS, THREAD_PARTICIPANTS, THREAD_OBJECTS, STACK, THREADS],
        Context::ThreadParticipants => &[THREAD_PARTICIPANTS, THREAD_OBJECTS, STACK, THREAD_OPERATIONS, THREADS],
        Context::ThreadObjects => &[THREAD_OBJECTS, STACK, THREAD_PARTICIPANTS, THREAD_OPERATIONS, THREADS],
        Context::RuntimeWorkers => &[RUNTIME_WORKERS, RUNTIME_TASKS, RUNTIME_DETAILS, RUNTIME_STACK],
        Context::RuntimeTasks => &[RUNTIME_TASKS, RUNTIME_DETAILS, RUNTIME_STACK, RUNTIME_WORKERS],
        Context::RuntimeDetails => &[RUNTIME_DETAILS, RUNTIME_TASKS, RUNTIME_STACK, RUNTIME_WORKERS],
        Context::RuntimeSpawnStack => &[RUNTIME_STACK, RUNTIME_DETAILS, RUNTIME_TASKS, RUNTIME_WORKERS],
        Context::IoResources => &[IO_RESOURCES, IO_OPERATIONS],
        Context::IoOperations => &[IO_OPERATIONS, IO_RESOURCES],
        Context::CacheTiers => &[CACHE_TIERS, CACHE_OPERATIONS],
        Context::CacheOperations => &[CACHE_OPERATIONS, CACHE_TIERS],
        Context::Recording => &[RECORDING],
        Context::Filters => &[FILTERS],
        Context::Capture => &[CAPTURE],
        Context::Loading => &[LOADING],
        Context::Error => &[ERROR],
    };
    let mut document = sections.to_vec();
    if matches!(
        context,
        Context::PrimitiveTypes
            | Context::PrimitiveOperations
            | Context::PrimitiveHotspots
            | Context::Threads
            | Context::ThreadOperations
            | Context::ThreadParticipants
            | Context::ThreadObjects
    ) {
        document.push(PRIMITIVE_VALUES);
    }
    if matches!(
        context,
        Context::RuntimeWorkers | Context::RuntimeTasks | Context::RuntimeDetails | Context::RuntimeSpawnStack
    ) {
        document.push(RUNTIME_STATES);
    }
    document.extend([COMMON, HELP_CONTROLS, if offline { OFFLINE } else { LIVE }]);
    document
}

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
    "Graph" => "Up to 120 approximately one-second samples. Horizontal axis: sample order. Vertical axis: events/second with an automatically scaled maximum.",
    "First sample / zero" => "The first sample establishes a baseline. Zero can mean idle, disabled/sampled recording or a reset counter, not zero application work.",
    "Live versus snapshot" => "Statistics refresh independently. A graph change does not refresh the manually captured analysis tables.",
);

const OFFLINE_INFO: &Section = section!("SNAPSHOT FILE";
    "Path" => "Native .seismograph file being inspected.",
    "Capture time" => "Capture time is not recorded in this format; loading time is not capture time.",
    "Source events" => "All-class accepted and overwritten source counts, plus the number of thread summaries; not an allocation population or a live rate.",
    "Heap errors" => "Heap decoding may fail while other telemetry remains available.",
    "Offline" => "There is no live activity polling or process connection. Scope notes apply to saved data.",
);

const HEAP_SUMMARY: &Section = section!("HEAP SUMMARY / TOPOLOGY / PEAKS";
    "Live" => "Allocator-reported live bytes / reported mapped bytes.",
    "Lifetime peak" => "Peak live bytes / current mapped bytes, only when the source explicitly provides lifetime scope.",
    "Max sampled live" => "Maximum of snapshot samples, not a guaranteed lifetime upper bound. Independent counter reads further limit consistency.",
    "Peak scope unavailable" => "The capture omitted peak scope; a lifetime interpretation is not justified.",
    "Gauge fill" => "Clamped to 0-100%; displayed values retain their actual numerator and denominator.",
    "Reported mapped (not RSS/committed)" => "Mapped bytes / reserved virtual bytes. Neither is portable committed memory or resident set size.",
    "Virtual regions" => "Region count, reserved bytes, slice size, and counts of small, medium, bump and other assigned slices.",
    "Rn / assigned / free" => "Region ID, reserved bytes and assigned/total virtual slices. Assignment is virtual, not physical residency.",
    "cumulative allocations" => "Counter epoch and coverage are unknown; not a session/workload delta. Availability is unencoded, so zero is not proof of no allocations.",
    "Filters" => "Whole-process counters and heap topology remain unfiltered.",
);

const HEAP_BUCKETS: &Section = section!("ALLOCATION TIERS / SIZE DISTRIBUTION";
    "Small / Medium / Large / Direct (inferred)" => "Allocation routing tiers. Large/Direct is inferred from size/alignment, not confirmed route metadata.",
    "Size" => "Allocation size or inclusive size range, in B/KiB/MiB/GiB.",
    "Retained" => "Retained allocation-event count in the bucket.",
    "Bytes" => "Sum of requested allocation bytes, not resident memory.",
    "Hotspots" => "Number of captured stack groups.",
    "Est. live" => "Small-tier published class live-allocation estimate when available; otherwise unmatched retained allocations.",
    "Est. class" => "Estimated live blocks / class capacity when available. Otherwise falls back to this bucket's retained allocation count / largest bucket count.",
    "Unmatched" => "Medium/Direct retained allocations without a matched retained free; not proven process-live allocations or leaks.",
    "Event share" => "Bucket retained allocation count / largest bucket count in the tier, NOT a percentage of the tier total.",
    "reported current" => "Small class count/byte estimates or Medium topology values. Small published classes do not cover all tiers; class estimates are not per-segment occupancy.",
    "Medium details" => "Virtual slice spans; overhead = usable minus requested bytes; largest = largest requested allocation.",
    "unmatched retained / retained" => "Direct current title values are unmatched retained count/bytes. retained title values sum allocation events, independently of topology.",
    "[ / ]" => "Change tier.",
    "Up/Down / Enter" => "Select a bucket / enter its locations. Nonfocusable summary and stack help appears on this same page.",
    "Missing events" => "No retained allocation events does not imply an empty heap.",
);

const HEAP_HOTSPOTS: &Section = section!("ALLOCATION LOCATIONS";
    "Events" => "Retained allocation count at this stack, within the selected size bucket.",
    "Bytes" => "Sum of requested allocation bytes at this stack.",
    "Unmatched" => "Count without paired retained frees. Even with zero overwrites, sampling, recording boundaries and missing frees prevent a process-live/leak conclusion.",
    "Location" => "First displayed frame of the captured stack.",
    "unmatched retained" => "Bucket-wide unmatched count/bytes in the title.",
    "class estimate" => "Estimated live blocks / class capacity from topology, not filtered stack totals.",
    "requested / waste" => "Estimated requested bytes / usable minus requested bytes.",
    "Up/Down / Backspace" => "Select a location and its stack / return to the size distribution.",
    "f / PgUp/PgDn" => "Toggle application/all frames / scroll the stack.",
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

const RUNTIME_WORKERS: &Section = section!("RUNTIME THREADS";
    "Runtime / thread" => "Runtime name and worker's recorder-thread ID.",
    "Role" => "Core executes general tasks; Blocking executes blocking work; Io drives runtime I/O.",
    "State" => "Running means available to execute, not necessarily polling now. Parked means parked; Stopped means stopped. Event-only workers can have blank metadata.",
    "Tasks" => "Tasks associated with this worker, not just currently running tasks. A migrated task can appear under multiple workers.",
    "Poll busy" => "Summed retained task-poll duration / time from worker's first to last retained runtime event, as a percentage. NOT process CPU utilization. Zero span yields zero; loss/filtering changes this incomplete window estimate.",
    "Avg poll" => "Summed retained poll time / retained poll count.",
    "Max poll" => "Largest retained poll duration. Worker metrics are retained-window values even when task counters are lifetime.",
    "unassigned / Unbound" => "Tasks with no worker association. '-' poll metrics are unavailable, not zero.",
    "runtime events / source retained" => "Retained runtime-class count / retained all-class source-event count.",
    "accepted / overwritten / loss %" => "All-class source totals. Loss percentage = overwritten / accepted; zero when accepted is zero.",
    "No runtime data" => "A recorder does not instrument executors automatically. Runtime source presence alone is not event history.",
    "Up/Down / Enter" => "Select worker / enter Tasks.",
);

const RUNTIME_TASKS: &Section = section!("RUNTIME TASKS";
    "Task" => "Runtime task ID.",
    "State" => "Reported/latest retained lifecycle state: Spawned, Materialized, Running, Pending, Completed, Canceled or Panicked. Pending means not polling, not proof the task is ready to run.",
    "Scope" => "lifetime: runtime-published task counters. retained window: reconstructed from retained events. Do not compare these as equal observation periods.",
    "Polls" => "Poll count in the row's Scope.",
    "Avg resume" => "Total poll-finish-to-next-poll-start gap / Resume samples. Includes normal I/O, timer or producer waiting; NOT scheduler stall.",
    "Max resume" => "Largest time from a poll finishing until the next poll starts.",
    "Avg stall" => "Total ready-wait duration / Ready-wait samples. Measured from first wake until the next poll begins, not time inside poll.",
    "Max stall" => "Largest wake-to-poll ready-wait duration.",
    "Units / zero" => "ns/us/ms/s. Averages with no samples display zero; inspect Scope and sample counts before concluding there is no delay.",
    "Up/Down / Enter / Backspace" => "Select task / enter Details / return to runtime threads.",
    "[ / ] / r" => "Change sort / reverse. Poll-duration sorts refer to values shown in Details.",
);

const RUNTIME_DETAILS: &Section = section!("TASK DETAILS";
    "Task / Runtime" => "Task ID and its runtime ID.",
    "Parent" => "Parent task ID; '-' means absent.",
    "Type descriptor" => "Recorded future/type descriptor ID; '-' means absent.",
    "Workers" => "Associated worker IDs, not necessarily OS thread IDs.",
    "Metric scope" => "Governs poll, resume and ready-wait samples, totals, averages and maxima. lifetime and retained window are different populations.",
    "Polls" => "Number of polls in the metric scope.",
    "Total poll time" => "Sum of execution time inside polls.",
    "Average poll duration" => "Total poll time / Polls.",
    "Maximum poll duration" => "Longest execution inside one poll.",
    "Resume samples" => "Number of measured inter-poll gaps.",
    "Average / Maximum time between polls" => "Average / largest gap from poll finish to next poll start. Includes normal task waiting.",
    "Ready-wait samples" => "Number of measured wake-to-poll intervals.",
    "Total scheduler stall" => "Sum of time from first wake until polling starts; not time inside poll.",
    "Average / Maximum scheduler stall" => "Total scheduler stall / Ready-wait samples, and largest such interval.",
    "Retained-window enqueues" => "Retained enqueue-event count, even when task metrics are lifetime.",
    "Retained-window materializations" => "Retained task materialization-event count.",
    "Retained-window transfer events" => "Recorded transfer/relocation phases, not necessarily complete migrations.",
    "Lifetime" => "Completed/canceled/panicked timestamp minus spawn timestamp. '-' if an endpoint is missing. Elapsed lifetime, not CPU time.",
    "Tab" => "While details are focused, toggle Details / Spawn Stack instead of changing main tab.",
    "PgUp/PgDn / Backspace" => "Scroll details / return to Tasks.",
);

const RUNTIME_STACK: &Section = section!("TASK SPAWN STACK";
    "Spawn Stack" => "Selected task's spawn-site backtrace, not its current execution or sampled poll stack.",
    "Frame number" => "Zero-based frame index.",
    "Backtrace not captured" => "Enable Runtime tasks backtraces before spawning new tasks. Existing tasks cannot acquire a spawn stack retroactively.",
    "Filters" => "Event stack or spawn provenance can select runtime records; spawn attribution does not imply execution at that site.",
    "Tab / PgUp/PgDn / Backspace" => "Toggle detail view / scroll / return to Tasks.",
);

const RUNTIME_STATES: &Section = section!("TASK STATE VALUES";
    "Spawned" => "A retained spawn event assigned the task identity.",
    "Materialized" => "A retained task-materialization event was observed.",
    "Running" => "Task is assigned as current by the runtime source, or its latest retained transition begins a poll.",
    "Pending" => "Not currently polling. This does not prove the task is ready to run; it may be waiting for a wake.",
    "Completed" => "A retained successful completion transition.",
    "Canceled" => "A retained cancellation transition.",
    "Panicked" => "A retained panic transition.",
    "Blank / incomplete state" => "Insufficient retained lifecycle information. Missing transitions limit event-only reconstruction.",
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
    "Unfiltered values" => "Source accepted/overwritten counters, whole-process counters and heap topology remain unfiltered.",
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
    "Snapshot buffers" => "retain keeps records; clear clears them; release releases recorder buffers. Windows may overlap or restart.",
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
    "Global versus filtered" => "Whole-process counters and heap topology remain unfiltered. Record totals can change with F filters; compare only matching scope and denominator.",
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
    "1-8 / Tab / Shift-Tab" => "Select/cycle main tabs. Runtime Details uses Tab to toggle its detail view instead.",
    "Click rows / drag borders" => "Select and enter a row's detail pane / resize panes.",
    "s" => "Capture a snapshot. Analysis tables do not continuously refresh.",
    "c" => "Configure recording.",
    "d" => "Cycle snapshot-buffer disposition.",
    "F" => "Edit whole-record stack filters.",
    "Esc / q" => "Disconnect to browser / quit.",
    "A/E/X/R/I/C" => "Footer indicators: Allocations, general Events, Arc dereferences, Runtime tasks, I/O, Cache. Letter = enabled; '-' = disabled. Sampling/backtrace settings are in Info or c.",
);

const OFFLINE: &Section = section!("OFFLINE NAVIGATION - AFTER CLOSING HELP";
    "Read-only" => "Offline is read-only: no process connection, recording changes, capture or buffer disposition changes.",
    "1-8 / Tab / Shift-Tab" => "Select/cycle tabs, except Runtime Details. Sorting, stack presentation and F filters change only the saved-data view.",
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
        Context::RuntimeDetails,
        Context::RuntimeSpawnStack,
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
    fn every_context_has_explicit_sections_entries_controls_and_scope() {
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
                for required in ["F1", "PgUp/PgDn", "Home/End", "denominator", "sampled-out", "remain unfiltered"] {
                    assert!(help.contains(required), "{context:?}: missing {required}");
                }
                assert!(help.contains(if offline { "Offline is read-only" } else { "LIVE NAVIGATION" }));
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
                "Size",
                "Retained",
                "Bytes",
                "Hotspots",
                "Est. live",
                "Est. class",
                "Unmatched",
                "Event share",
                "Live",
                "Lifetime peak",
                "Max sampled live",
                "Reported mapped (not RSS/committed)",
                "Virtual regions",
                "cumulative allocations",
            ],
        ),
        (
            Context::HeapHotspots,
            &["Events", "Bytes", "Unmatched", "Location", "class estimate"],
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
            &["Runtime / thread", "Role", "State", "Tasks", "Poll busy", "Avg poll", "Max poll"],
        ),
        (
            Context::RuntimeTasks,
            &[
                "Task",
                "State",
                "Scope",
                "Polls",
                "Avg resume",
                "Max resume",
                "Avg stall",
                "Max stall",
            ],
        ),
        (
            Context::RuntimeDetails,
            &[
                "Parent",
                "Type descriptor",
                "Workers",
                "Metric scope",
                "Polls",
                "Total poll time",
                "Average poll duration",
                "Maximum poll duration",
                "Resume samples",
                "Ready-wait samples",
                "Retained-window enqueues",
                "Retained-window materializations",
                "Retained-window transfer events",
                "Lifetime",
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
    fn help_explains_noninterchangeable_scopes_and_operation_values() {
        for (context, distinctions) in [
            (
                Context::RuntimeTasks,
                &[
                    "lifetime",
                    "retained window",
                    "poll finishing",
                    "first wake",
                    "NOT scheduler stall",
                    "not time inside poll",
                    "no samples",
                    "Spawned",
                    "Materialized",
                    "Pending",
                    "Completed",
                    "Canceled",
                    "Panicked",
                ][..],
            ),
            (
                Context::RuntimeWorkers,
                &[
                    "first to last retained",
                    "NOT process CPU",
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
                    "not a guaranteed lifetime",
                    "not per-segment occupancy",
                    "not physical residency",
                    "NOT a percentage of the tier total",
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

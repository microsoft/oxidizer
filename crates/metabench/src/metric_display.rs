// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TableMetric {
    pub(crate) engine: String,
    pub(crate) key: String,
    pub(crate) label: Cow<'static, str>,
    pub(crate) priority: u8,
    rank: u8,
}

impl TableMetric {
    pub(crate) fn new(engine: String, key: String) -> Self {
        let (label, priority, rank) = descriptor(&format!("{engine}/{key}"));
        Self {
            engine,
            key,
            label,
            priority,
            rank,
        }
    }

    pub(crate) fn sort_key(&self) -> (u8, u8, &str, &str) {
        (self.priority, self.rank, &self.engine, &self.key)
    }
}

fn descriptor(key: &str) -> (Cow<'static, str>, u8, u8) {
    let known = match key {
        "criterion/median" => ("Time", 1, 1),
        "alloc_tracker/Allocations" => ("Allocs", 1, 12),
        "alloc_tracker/Allocated bytes" => ("Bytes", 1, 13),
        "gungraun.callgrind/Ir" => ("Instr", 1, 2),
        "gungraun.cachegrind/Ir" => ("Instr", 1, 3),
        "gungraun.callgrind/EstimatedCycles" => ("Cycles", 1, 4),
        "perf/instructions" => ("HW Instr", 1, 5),
        "perf/cycles" => ("HW Cycles", 1, 6),
        "criterion/declared_throughput.bits" => ("Bits", 1, 7),
        "criterion/declared_throughput.bytes" => ("Bytes", 1, 8),
        "criterion/declared_throughput.bytes_decimal" => ("Decimal Bytes", 1, 9),
        "criterion/declared_throughput.elements" => ("Elements", 1, 10),
        "criterion/declared_throughput.elements_and_bytes" => ("Elements And Bytes", 1, 11),

        "gungraun.dhat/TotalBytes" => ("DHAT Bytes", 2, 1),
        "gungraun.dhat/TotalBlocks" => ("DHAT Blocks", 2, 2),
        "gungraun.dhat/MaximumBytes" => ("DHAT Peak Bytes", 2, 3),
        "gungraun.dhat/MaximumBlocks" => ("DHAT Peak Blocks", 2, 4),

        "gungraun.callgrind/I1mr" => ("L1 I Miss", 3, 1),
        "gungraun.cachegrind/I1mr" => ("L1 I Miss", 3, 2),
        "gungraun.callgrind/D1mr" => ("L1 D-R Miss", 3, 3),
        "gungraun.cachegrind/D1mr" => ("L1 D-R Miss", 3, 4),
        "gungraun.callgrind/D1mw" => ("L1 D-W Miss", 3, 5),
        "gungraun.cachegrind/D1mw" => ("L1 D-W Miss", 3, 6),
        "gungraun.callgrind/Bcm" => ("Branch Miss", 3, 7),
        "gungraun.callgrind/Bim" => ("Indirect Branch Miss", 3, 8),
        "gungraun.callgrind/Bc" => ("Conditional Branches", 5, 9),
        "gungraun.callgrind/Bi" => ("Indirect Branches", 5, 10),
        "perf/branch-misses" => ("HW Branch Miss", 3, 11),
        "gungraun.callgrind/ILmr" => ("Last-Level I Miss", 3, 12),
        "gungraun.cachegrind/ILmr" => ("Last-Level I Miss", 3, 13),
        "gungraun.callgrind/DLmr" => ("Last-Level D-R Miss", 3, 14),
        "gungraun.cachegrind/DLmr" => ("Last-Level D-R Miss", 3, 15),
        "gungraun.callgrind/DLmw" => ("Last-Level D-W Miss", 3, 16),
        "gungraun.cachegrind/DLmw" => ("Last-Level D-W Miss", 3, 17),
        "perf/cache-misses" => ("HW Cache Miss", 3, 18),

        "gungraun.callgrind/I1MissRate" => ("L1 I Miss Rate", 4, 1),
        "gungraun.cachegrind/I1MissRate" => ("L1 I Miss Rate", 4, 2),
        "gungraun.callgrind/D1MissRate" => ("L1 D Miss Rate", 4, 3),
        "gungraun.cachegrind/D1MissRate" => ("L1 D Miss Rate", 4, 4),
        "gungraun.callgrind/LLiMissRate" => ("Last-Level I Miss Rate", 4, 5),
        "gungraun.cachegrind/LLiMissRate" => ("Last-Level I Miss Rate", 4, 6),
        "gungraun.callgrind/LLdMissRate" => ("Last-Level D Miss Rate", 4, 7),
        "gungraun.cachegrind/LLdMissRate" => ("Last-Level D Miss Rate", 4, 8),
        "gungraun.callgrind/LLMissRate" => ("Last-Level Cache Miss Rate", 4, 9),
        "gungraun.cachegrind/LLMissRate" => ("Last-Level Cache Miss Rate", 4, 10),
        "gungraun.callgrind/L1HitRate" => ("L1 Cache Hit Rate", 4, 11),
        "gungraun.cachegrind/L1HitRate" => ("L1 Cache Hit Rate", 4, 12),
        "gungraun.callgrind/LLHitRate" => ("Last-Level Cache Hit Rate", 4, 13),
        "gungraun.cachegrind/LLHitRate" => ("Last-Level Cache Hit Rate", 4, 14),
        "gungraun.callgrind/RamHitRate" => ("RAM Hit Rate", 4, 15),
        "gungraun.cachegrind/RamHitRate" => ("RAM Hit Rate", 4, 16),

        "gungraun.callgrind/Dr" => ("Data Reads", 5, 1),
        "gungraun.cachegrind/Dr" => ("Data Reads", 5, 2),
        "gungraun.callgrind/Dw" => ("Data Writes", 5, 3),
        "gungraun.cachegrind/Dw" => ("Data Writes", 5, 4),
        "gungraun.callgrind/TotalRW" => ("Total Data Accesses", 5, 5),
        "gungraun.cachegrind/TotalRW" => ("Total Data Accesses", 5, 6),
        "gungraun.dhat/ReadsBytes" => ("DHAT Bytes Read", 5, 9),
        "gungraun.dhat/WritesBytes" => ("DHAT Bytes Written", 5, 10),
        "perf/branches" => ("HW Branches", 5, 11),
        "perf/cache-references" => ("HW Cache References", 5, 12),

        "gungraun.callgrind/L1hits" => ("L1 Cache Hits", 6, 1),
        "gungraun.cachegrind/L1hits" => ("L1 Cache Hits", 6, 2),
        "gungraun.callgrind/LLhits" => ("Last-Level Cache Hits", 6, 3),
        "gungraun.cachegrind/LLhits" => ("Last-Level Cache Hits", 6, 4),
        "gungraun.callgrind/RamHits" => ("RAM Accesses", 6, 5),
        "gungraun.cachegrind/RamHits" => ("RAM Accesses", 6, 6),
        "gungraun.callgrind/SysCount" => ("System Calls", 6, 7),
        "gungraun.callgrind/SysTime" => ("System-Call Elapsed Time", 6, 8),
        "gungraun.callgrind/SysCpuTime" => ("System-Call CPU Time", 6, 9),
        "gungraun.callgrind/Ge" => ("Global Bus Events", 6, 10),

        "gungraun.dhat/AtTGmaxBytes" => ("DHAT Bytes Alive At Heap Peak", 7, 1),
        "gungraun.dhat/AtTGmaxBlocks" => ("DHAT Blocks Alive At Heap Peak", 7, 2),
        "gungraun.dhat/AtTEndBytes" => ("DHAT Bytes Alive At Exit", 7, 3),
        "gungraun.dhat/AtTEndBlocks" => ("DHAT Blocks Alive At Exit", 7, 4),
        "gungraun.dhat/TotalLifetimes" => ("DHAT Total Block Lifetimes", 7, 5),

        "gungraun.callgrind/ILdmr" => ("Dirty Instruction Misses", 8, 1),
        "gungraun.callgrind/DLdmr" => ("Dirty Data Read Misses", 8, 2),
        "gungraun.callgrind/DLdmw" => ("Dirty Data Write Misses", 8, 3),

        "gungraun.callgrind/AcCost1" => ("L1 Temporal Locality Cost", 9, 1),
        "gungraun.callgrind/AcCost2" => ("Last-Level Temporal Locality Cost", 9, 2),
        "gungraun.callgrind/SpLoss1" => ("L1 Spatial Locality Loss", 9, 3),
        "gungraun.callgrind/SpLoss2" => ("Last-Level Spatial Locality Loss", 9, 4),

        "gungraun.dhat/TotalUnits" => ("DHAT Ad-Hoc Units", 10, 1),
        "gungraun.dhat/TotalEvents" => ("DHAT Ad-Hoc Events", 10, 2),

        _ => {
            let name = key.rsplit_once('/').map_or(key, |(_, name)| name);
            if key.starts_with("vtune/") {
                // VTune hardware event names (for example `INST_RETIRED.ANY`) are
                // already SCREAMING_SNAKE_CASE, so `humanize`'s "insert a space
                // before every uppercase letter" heuristic (built for names like
                // `TotalBytes`) would shred them into single letters. Only join
                // the existing word separators.
                return (Cow::Owned(name.replace(['_', '.'], " ")), 10, u8::MAX);
            }
            return (Cow::Owned(title_case(&humanize(name))), 10, u8::MAX);
        }
    };
    (Cow::Borrowed(known.0), known.1, known.2)
}

fn humanize(key: &str) -> String {
    let mut output = String::with_capacity(key.len());
    for (index, character) in key.chars().enumerate() {
        if character == '_' || character == '.' {
            output.push(' ');
        } else {
            if character.is_uppercase() && index != 0 {
                output.push(' ');
            }
            output.push(character);
        }
    }
    output
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            characters
                .next()
                .map_or_else(String::new, |first| first.to_uppercase().chain(characters).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::TableMetric;

    #[test]
    fn known_and_unknown_metrics_have_stable_presentations() {
        let instructions = TableMetric::new("gungraun.callgrind".to_owned(), "Ir".to_owned());
        assert_eq!(instructions.label, "Instr");
        assert_eq!(instructions.priority, 1);

        let unknown = TableMetric::new("custom_engine".to_owned(), "custom_metric".to_owned());
        assert_eq!(unknown.label, "Custom Metric");
        assert_eq!(unknown.priority, 10);

        assert_eq!(
            TableMetric::new("gungraun.dhat".to_owned(), "TotalBytes".to_owned()).label,
            "DHAT Bytes"
        );
        assert_eq!(
            TableMetric::new("criterion".to_owned(), "declared_throughput.elements_and_bytes".to_owned()).label,
            "Elements And Bytes"
        );
    }

    #[test]
    fn vtune_metrics_preserve_screaming_snake_case_words() {
        let metric = TableMetric::new("vtune".to_owned(), "INST_RETIRED.ANY".to_owned());
        assert_eq!(metric.label, "INST RETIRED ANY");
        assert_eq!(metric.priority, 10);
    }
}

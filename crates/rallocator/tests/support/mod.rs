// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Stats {
    pub(crate) allocated_bytes: usize,
    pub(crate) deallocated_bytes: usize,
    pub(crate) live_bytes: usize,
    pub(crate) mapped_bytes: usize,
    pub(crate) os_mappings: usize,
    pub(crate) os_unmappings: usize,
    pub(crate) allocations: usize,
    pub(crate) deallocations: usize,
}

pub(crate) fn stats() -> Option<Stats> {
    let _suppression = seismograph::recorder::SuppressionGuard::enter();
    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).ok()?;
    let mut reader = Reader::new(encoded.as_bytes());
    reader.skip(8 + 2 + 2 + 8)?;
    let thread_count = reader.u32()? as usize;
    let event_count = reader.u32()? as usize;
    let source_count = reader.u32()? as usize;
    reader.skip(6 * 8 + 2 + 2 + 8)?;
    for _ in 0..thread_count {
        reader.skip(24)?;
        let name_len = reader.u16()? as usize;
        reader.skip(name_len)?;
    }
    for _ in 0..event_count {
        reader.skip(24 + 2)?;
        let frame_count = reader.u8()? as usize;
        reader.skip(1 + 8 * 8 + frame_count * 8)?;
    }
    let mut source_data = None;
    for _ in 0..source_count {
        let id = reader.u64()?;
        reader.skip(2)?;
        let name_len = reader.u16()? as usize;
        let data_len = usize::try_from(reader.u64()?).ok()?;
        reader.skip(4)?;
        reader.skip(name_len)?;
        let data = reader.read(data_len)?;
        if id == seismograph_rallocator::source::ID.get() {
            source_data = Some(data);
        }
    }
    let mut source = Reader::new(source_data?);
    source.skip(20)?;
    source.skip(8 + 8)?;
    if source.u16()? != 2 {
        return None;
    }
    source.skip(2)?;
    if source.u32()? != 13 * 8 {
        return None;
    }
    let allocated_bytes = source.u64()?;
    let deallocated_bytes = source.u64()?;
    let live_bytes = source.u64()?;
    source.skip(8)?;
    let mapped_bytes = source.u64()?;
    let os_mappings = source.u64()?;
    let os_unmappings = source.u64()?;
    let allocations = source.u64()?;
    let deallocations = source.u64()?;
    Some(Stats {
        allocated_bytes: usize::try_from(allocated_bytes).ok()?,
        deallocated_bytes: usize::try_from(deallocated_bytes).ok()?,
        live_bytes: usize::try_from(live_bytes).ok()?,
        mapped_bytes: usize::try_from(mapped_bytes).ok()?,
        os_mappings: usize::try_from(os_mappings).ok()?,
        os_unmappings: usize::try_from(os_unmappings).ok()?,
        allocations: usize::try_from(allocations).ok()?,
        deallocations: usize::try_from(deallocations).ok()?,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn read(&mut self, len: usize) -> Option<&'a [u8]> {
        let (value, remaining) = self.bytes.split_at_checked(len)?;
        self.bytes = remaining;
        Some(value)
    }

    fn skip(&mut self, len: usize) -> Option<()> {
        self.read(len).map(|_| ())
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.read(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.read(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.read(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.read(8)?.try_into().ok()?))
    }
}

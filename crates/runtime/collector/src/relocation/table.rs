//! Deterministic segmented storage for opaque managed tokens.

use std::collections::VecDeque;

use pop_runtime_interface::ManagedReference;

const SEGMENT_LENGTH: usize = 256;
type SegmentEntry<Value> = Option<Value>;
type Segment<Value> = Box<[SegmentEntry<Value>]>;
type SegmentWindow<Value> = VecDeque<Option<Segment<Value>>>;

#[derive(Clone, Debug)]
pub(crate) struct ObjectTable<Value> {
    base_segment: Option<u64>,
    segments: SegmentWindow<Value>,
    length: usize,
}

impl<Value> ObjectTable<Value> {
    pub(crate) const fn new() -> Self {
        Self {
            base_segment: None,
            segments: VecDeque::new(),
            length: 0,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.length
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.length == 0
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn contains_key(&self, reference: &ManagedReference) -> bool {
        self.get(reference).is_some()
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn get(&self, reference: &ManagedReference) -> Option<&Value> {
        let (segment, offset) = coordinates(*reference)?;
        let index = self.segment_index(segment)?;
        self.segments.get(index)?.as_ref()?.get(offset)?.as_ref()
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn get_mut(&mut self, reference: &ManagedReference) -> Option<&mut Value> {
        let (segment, offset) = coordinates(*reference)?;
        let index = self.segment_index(segment)?;
        self.segments
            .get_mut(index)?
            .as_mut()?
            .get_mut(offset)?
            .as_mut()
    }

    pub(crate) fn insert(&mut self, reference: ManagedReference, value: Value) -> Option<Value> {
        let (segment, offset) = coordinates(reference).expect("managed references are nonzero");
        let index = self.ensure_segment_index(segment);
        let entries = self.segments[index].get_or_insert_with(empty_segment);
        let previous = entries[offset].replace(value);
        if previous.is_none() {
            self.length = self.length.saturating_add(1);
        }
        previous
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn remove(&mut self, reference: &ManagedReference) -> Option<Value> {
        let (segment, offset) = coordinates(*reference)?;
        let index = self.segment_index(segment)?;
        let (removed, empty) = {
            let entries = self.segments.get_mut(index)?.as_mut()?;
            let removed = entries.get_mut(offset)?.take();
            let empty = removed.is_some() && entries.iter().all(Option::is_none);
            (removed, empty)
        };
        if removed.is_some() {
            self.length = self.length.saturating_sub(1);
        }
        if empty {
            self.segments[index] = None;
            self.trim_empty_edges();
        }
        removed
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (ManagedReference, &Value)> {
        let base = self.base_segment.unwrap_or(0);
        self.segments
            .iter()
            .enumerate()
            .filter_map(|(relative, segment)| segment.as_ref().map(|entries| (relative, entries)))
            .flat_map(move |(relative, entries)| {
                entries
                    .iter()
                    .enumerate()
                    .filter_map(move |(offset, entry)| {
                        entry.as_ref().map(|value| {
                            (
                                reference_at(base, relative, offset)
                                    .expect("stored segment coordinate is representable"),
                                value,
                            )
                        })
                    })
            })
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (ManagedReference, &mut Value)> {
        let base = self.base_segment.unwrap_or(0);
        self.segments
            .iter_mut()
            .enumerate()
            .filter_map(|(relative, segment)| segment.as_mut().map(|entries| (relative, entries)))
            .flat_map(move |(relative, entries)| {
                entries
                    .iter_mut()
                    .enumerate()
                    .filter_map(move |(offset, entry)| {
                        entry.as_mut().map(|value| {
                            (
                                reference_at(base, relative, offset)
                                    .expect("stored segment coordinate is representable"),
                                value,
                            )
                        })
                    })
            })
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &Value> {
        self.iter().map(|(_, value)| value)
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        self.iter_mut().map(|(_, value)| value)
    }

    pub(crate) fn next_after(
        &self,
        cursor: Option<ManagedReference>,
    ) -> Option<(ManagedReference, &Value)> {
        let first_raw = match cursor {
            Some(reference) => reference.raw().checked_add(1)?,
            None => 1,
        };
        let first = ManagedReference::new(first_raw);
        let (first_segment, first_offset) = coordinates(first)?;
        let base = self.base_segment?;
        let start_segment = first_segment.max(base);
        let start_index = usize::try_from(start_segment.checked_sub(base)?).ok()?;
        for (relative, entries) in self.segments.iter().enumerate().skip(start_index) {
            let segment = base.checked_add(u64::try_from(relative).ok()?)?;
            let Some(entries) = entries.as_ref() else {
                continue;
            };
            let offset = if segment == first_segment {
                first_offset
            } else {
                0
            };
            if let Some((found, value)) = entries[offset..]
                .iter()
                .enumerate()
                .find_map(|(relative, entry)| entry.as_ref().map(|value| (relative, value)))
            {
                return Some((
                    reference_at(base, usize::try_from(segment - base).ok()?, offset + found)?,
                    value,
                ));
            }
        }
        None
    }

    fn segment_index(&self, segment: u64) -> Option<usize> {
        let base = self.base_segment?;
        let index = usize::try_from(segment.checked_sub(base)?).ok()?;
        (index < self.segments.len()).then_some(index)
    }

    fn ensure_segment_index(&mut self, segment: u64) -> usize {
        let Some(mut base) = self.base_segment else {
            self.base_segment = Some(segment);
            self.segments.push_back(None);
            return 0;
        };
        while segment < base {
            self.segments.push_front(None);
            base -= 1;
        }
        self.base_segment = Some(base);
        let index = usize::try_from(segment - base).expect("token segment index fits usize");
        if self.segments.len() <= index {
            self.segments.resize_with(index + 1, || None);
        }
        index
    }

    fn trim_empty_edges(&mut self) {
        while self.segments.front().is_some_and(Option::is_none) {
            self.segments.pop_front();
            self.base_segment = self.base_segment.and_then(|base| base.checked_add(1));
        }
        while self.segments.back().is_some_and(Option::is_none) {
            self.segments.pop_back();
        }
        if self.segments.is_empty() {
            self.base_segment = None;
        }
    }
}

impl<Value> Default for ObjectTable<Value> {
    fn default() -> Self {
        Self::new()
    }
}

fn coordinates(reference: ManagedReference) -> Option<(u64, usize)> {
    let index = reference.raw().checked_sub(1)?;
    let segment_length = u64::try_from(SEGMENT_LENGTH).ok()?;
    let segment = index / segment_length;
    let offset = usize::try_from(index % segment_length).ok()?;
    Some((segment, offset))
}

fn reference_at(base: u64, relative: usize, offset: usize) -> Option<ManagedReference> {
    let segment = base.checked_add(u64::try_from(relative).ok()?)?;
    let segment_length = u64::try_from(SEGMENT_LENGTH).ok()?;
    let raw = segment
        .checked_mul(segment_length)?
        .checked_add(u64::try_from(offset).ok()?)?
        .checked_add(1)?;
    Some(ManagedReference::new(raw))
}

fn empty_segment<Value>() -> Segment<Value> {
    std::iter::repeat_with(|| None)
        .take(SEGMENT_LENGTH)
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(raw: u64) -> ManagedReference {
        ManagedReference::new(raw)
    }

    #[test]
    fn segment_entries_store_only_the_value_payload() {
        assert_eq!(
            std::mem::size_of::<SegmentEntry<u64>>(),
            std::mem::size_of::<Option<u64>>()
        );
    }

    #[test]
    fn segmented_table_preserves_exact_token_order_across_segments() {
        let mut table = ObjectTable::new();
        table.insert(reference(1_024), "fourth");
        table.insert(reference(257), "third");
        table.insert(reference(1), "first");
        table.insert(reference(256), "second");

        assert_eq!(table.len(), 4);
        assert_eq!(
            table
                .iter()
                .map(|(reference, value)| (reference.raw(), *value))
                .collect::<Vec<_>>(),
            vec![
                (1, "first"),
                (256, "second"),
                (257, "third"),
                (1_024, "fourth")
            ]
        );
        assert_eq!(
            table
                .next_after(Some(reference(256)))
                .map(|entry| entry.0.raw()),
            Some(257)
        );
    }

    #[test]
    fn segmented_table_removes_empty_segments_and_skips_their_token_ranges() {
        let mut table = ObjectTable::new();
        table.insert(reference(1), 10);
        table.insert(reference(256), 20);
        table.insert(reference(257), 30);

        assert_eq!(table.remove(&reference(1)), Some(10));
        assert_eq!(table.remove(&reference(256)), Some(20));
        assert!(!table.contains_key(&reference(1)));
        assert_eq!(table.next_after(None).map(|entry| entry.0.raw()), Some(257));
        assert_eq!(table.remove(&reference(257)), Some(30));
        assert_eq!(table.len(), 0);
        assert!(table.next_after(None).is_none());
    }
}

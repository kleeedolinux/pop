//! Inline bootstrap payloads and page-backed generational payload views.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pop_runtime_interface::ManagedReference;

/// One physical payload word.
///
/// The allocation's precise object map, rather than a duplicated per-slot tag,
/// determines whether this word is a scalar or a managed reference.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub(crate) struct SlotValue(u64);

impl SlotValue {
    pub(crate) const fn scalar(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn reference(value: Option<ManagedReference>) -> Self {
        Self(match value {
            Some(reference) => reference.raw(),
            None => 0,
        })
    }

    pub(crate) const fn raw(self) -> u64 {
        self.0
    }

    pub(crate) const fn as_reference(self) -> Option<ManagedReference> {
        if self.0 == 0 {
            None
        } else {
            Some(ManagedReference::new(self.0))
        }
    }
}

pub(crate) type PageWords = Arc<[AtomicU64]>;

// Fixed default pages let LLVM lower initialization to one zero fill. The
// temporary is bounded to 32 KiB and immediately moved into retained storage.
#[allow(clippy::large_stack_arrays)]
pub(crate) fn zeroed_page_words(length: usize) -> PageWords {
    match length {
        32 => Arc::new([const { AtomicU64::new(0) }; 32]),
        512 => Arc::new([const { AtomicU64::new(0) }; 512]),
        4096 => Arc::new([const { AtomicU64::new(0) }; 4096]),
        _ => (0..length)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into(),
    }
}

const INLINE_SLOT_CAPACITY: usize = 2;

pub(crate) enum SlotStorage {
    Inline {
        length: u8,
        values: [SlotValue; INLINE_SLOT_CAPACITY],
    },
    Heap(Vec<SlotValue>),
    PageBacked {
        words: PageWords,
        start: usize,
        length: usize,
    },
}

impl SlotStorage {
    pub(crate) const fn new() -> Self {
        Self::Inline {
            length: 0,
            values: [SlotValue::scalar(0); INLINE_SLOT_CAPACITY],
        }
    }

    pub(crate) const fn len(&self) -> usize {
        match self {
            Self::Inline { length, .. } => *length as usize,
            Self::Heap(values) => values.len(),
            Self::PageBacked { length, .. } => *length,
        }
    }

    pub(crate) const fn is_page_backed(&self) -> bool {
        matches!(self, Self::PageBacked { .. })
    }

    pub(crate) fn from_page_values(
        words: PageWords,
        start: usize,
        values: &[u64],
    ) -> Result<Self, ()> {
        let end = start.checked_add(values.len()).ok_or(())?;
        let target = words.get(start..end).ok_or(())?;
        for (word, value) in target.iter().zip(values.iter().copied()) {
            word.store(value, Ordering::Relaxed);
        }
        Ok(Self::PageBacked {
            words,
            start,
            length: values.len(),
        })
    }

    pub(crate) fn page_range_is_valid(words: &PageWords, start: usize, length: usize) -> bool {
        start
            .checked_add(length)
            .is_some_and(|end| words.get(start..end).is_some())
    }

    pub(crate) fn from_validated_page_range(words: PageWords, start: usize, length: usize) -> Self {
        debug_assert!(Self::page_range_is_valid(&words, start, length));
        Self::PageBacked {
            words,
            start,
            length,
        }
    }

    pub(crate) fn from_page_fill(
        words: PageWords,
        start: usize,
        length: usize,
        value: u64,
    ) -> Result<Self, ()> {
        let end = start.checked_add(length).ok_or(())?;
        let target = words.get(start..end).ok_or(())?;
        for word in target {
            word.store(value, Ordering::Relaxed);
        }
        Ok(Self::PageBacked {
            words,
            start,
            length,
        })
    }

    pub(crate) fn try_reserve_exact(
        &mut self,
        additional: usize,
    ) -> Result<(), std::collections::TryReserveError> {
        if matches!(self, Self::PageBacked { .. }) {
            *self = Self::from(self.values().collect::<Vec<_>>());
        }
        let required = self.len().saturating_add(additional);
        if required <= INLINE_SLOT_CAPACITY {
            return Ok(());
        }
        if let Self::Inline { length, values } = self {
            let length = usize::from(*length);
            let mut heap = Vec::new();
            heap.try_reserve_exact(required)?;
            heap.extend_from_slice(&values[..length]);
            *self = Self::Heap(heap);
        } else if let Self::Heap(heap) = self {
            heap.try_reserve_exact(additional)?;
        }
        Ok(())
    }

    pub(crate) fn push(&mut self, value: SlotValue) {
        match self {
            Self::Inline { length, values } if usize::from(*length) < INLINE_SLOT_CAPACITY => {
                values[usize::from(*length)] = value;
                *length += 1;
            }
            Self::Inline { .. } | Self::PageBacked { .. } => {
                self.try_reserve_exact(1)
                    .expect("slot storage was reserved before mutation");
                self.push(value);
            }
            Self::Heap(heap) => heap.push(value),
        }
    }

    pub(crate) fn get(&self, index: usize) -> Option<SlotValue> {
        match self {
            Self::Inline { length, values } => {
                (index < usize::from(*length)).then(|| values[index])
            }
            Self::Heap(values) => values.get(index).copied(),
            Self::PageBacked {
                words,
                start,
                length,
            } => (index < *length)
                .then(|| words[start + index].load(Ordering::Relaxed))
                .map(SlotValue::scalar),
        }
    }

    pub(crate) fn set(&mut self, index: usize, value: SlotValue) -> bool {
        match self {
            Self::Inline { length, values } if index < usize::from(*length) => {
                values[index] = value;
                true
            }
            Self::Heap(values) if index < values.len() => {
                values[index] = value;
                true
            }
            Self::PageBacked {
                words,
                start,
                length,
            } if index < *length => {
                words[*start + index].store(value.raw(), Ordering::Relaxed);
                true
            }
            Self::Inline { .. } | Self::Heap(_) | Self::PageBacked { .. } => false,
        }
    }

    pub(crate) fn fill(&mut self, value: SlotValue) {
        match self {
            Self::Inline { length, values } => values[..usize::from(*length)].fill(value),
            Self::Heap(values) => values.fill(value),
            Self::PageBacked {
                words,
                start,
                length,
            } => {
                for word in &words[*start..*start + *length] {
                    word.store(value.raw(), Ordering::Relaxed);
                }
            }
        }
    }

    pub(crate) fn values(&self) -> SlotValues<'_> {
        match self {
            Self::Inline { length, values } => {
                SlotValues::Owned(values[..usize::from(*length)].iter())
            }
            Self::Heap(values) => SlotValues::Owned(values.iter()),
            Self::PageBacked {
                words,
                start,
                length,
            } => SlotValues::Page {
                words,
                next: *start,
                end: *start + *length,
            },
        }
    }

    pub(crate) fn iter(&self) -> SlotValues<'_> {
        self.values()
    }

    pub(crate) fn bind_to_page(&mut self, words: PageWords, start: usize) -> Result<(), ()> {
        let length = self.len();
        let end = start.checked_add(length).ok_or(())?;
        if end > words.len() {
            return Err(());
        }
        match self {
            Self::Inline {
                length: inline_length,
                values,
            } => {
                for index in 0..usize::from(*inline_length) {
                    words[start + index].store(values[index].raw(), Ordering::Relaxed);
                }
            }
            Self::Heap(values) => {
                for (word, value) in words[start..end].iter().zip(values) {
                    word.store(value.raw(), Ordering::Relaxed);
                }
            }
            Self::PageBacked {
                words: source,
                start: source_start,
                ..
            } => {
                for index in 0..length {
                    words[start + index].store(
                        source[*source_start + index].load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                }
            }
        }
        *self = Self::PageBacked {
            words,
            start,
            length,
        };
        Ok(())
    }

    #[cfg(test)]
    const fn uses_heap_storage(&self) -> bool {
        matches!(self, Self::Heap(_))
    }
}

impl Clone for SlotStorage {
    fn clone(&self) -> Self {
        Self::from(self.values().collect::<Vec<_>>())
    }
}

impl std::fmt::Debug for SlotStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_list().entries(self.values()).finish()
    }
}

impl PartialEq for SlotStorage {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.values().eq(other.values())
    }
}

impl Eq for SlotStorage {}

impl Default for SlotStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<SlotValue>> for SlotStorage {
    fn from(values: Vec<SlotValue>) -> Self {
        if values.len() <= INLINE_SLOT_CAPACITY {
            let mut inline = [SlotValue::scalar(0); INLINE_SLOT_CAPACITY];
            inline[..values.len()].copy_from_slice(&values);
            Self::Inline {
                length: u8::try_from(values.len()).expect("inline slot length fits u8"),
                values: inline,
            }
        } else {
            Self::Heap(values)
        }
    }
}

pub(crate) enum SlotValues<'a> {
    Owned(std::slice::Iter<'a, SlotValue>),
    Page {
        words: &'a [AtomicU64],
        next: usize,
        end: usize,
    },
}

impl Iterator for SlotValues<'_> {
    type Item = SlotValue;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Owned(values) => values.next().copied(),
            Self::Page { words, next, end } if *next < *end => {
                let value = words[*next].load(Ordering::Relaxed);
                *next += 1;
                Some(SlotValue::scalar(value))
            }
            Self::Page { .. } => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let length = match self {
            Self::Owned(values) => values.len(),
            Self::Page { next, end, .. } => end - next,
        };
        (length, Some(length))
    }
}

impl ExactSizeIterator for SlotValues<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_slot_payloads_remain_inline() {
        let slots = SlotStorage::from(vec![SlotValue::scalar(1), SlotValue::scalar(2)]);

        assert!(!slots.uses_heap_storage());
        assert_eq!(
            slots.values().collect::<Vec<_>>(),
            [SlotValue::scalar(1), SlotValue::scalar(2)]
        );
    }

    #[test]
    fn physical_slots_use_exactly_one_machine_word() {
        assert_eq!(std::mem::size_of::<SlotValue>(), std::mem::size_of::<u64>());
    }
}

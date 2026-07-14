//! Inline small-payload storage behind the logical collector slot contract.

use std::ops::{Deref, DerefMut};

use pop_runtime_interface::ManagedReference;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SlotValue {
    Scalar(u64),
    Reference(Option<ManagedReference>),
}

const INLINE_SLOT_CAPACITY: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SlotStorage {
    Inline {
        length: u8,
        values: [SlotValue; INLINE_SLOT_CAPACITY],
    },
    Heap(Vec<SlotValue>),
}

impl SlotStorage {
    pub(crate) const fn new() -> Self {
        Self::Inline {
            length: 0,
            values: [SlotValue::Scalar(0); INLINE_SLOT_CAPACITY],
        }
    }

    pub(crate) fn try_reserve_exact(
        &mut self,
        additional: usize,
    ) -> Result<(), std::collections::TryReserveError> {
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
            Self::Inline { .. } => {
                self.try_reserve_exact(1)
                    .expect("slot storage was reserved before mutation");
                self.push(value);
            }
            Self::Heap(heap) => heap.push(value),
        }
    }

    #[cfg(test)]
    const fn uses_heap_storage(&self) -> bool {
        matches!(self, Self::Heap(_))
    }
}

impl Default for SlotStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<SlotValue>> for SlotStorage {
    fn from(values: Vec<SlotValue>) -> Self {
        if values.len() <= INLINE_SLOT_CAPACITY {
            let mut inline = [SlotValue::Scalar(0); INLINE_SLOT_CAPACITY];
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

impl Deref for SlotStorage {
    type Target = [SlotValue];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Inline { length, values } => &values[..usize::from(*length)],
            Self::Heap(values) => values,
        }
    }
}

impl DerefMut for SlotStorage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Inline { length, values } => &mut values[..usize::from(*length)],
            Self::Heap(values) => values,
        }
    }
}

impl<'a> IntoIterator for &'a SlotStorage {
    type Item = &'a SlotValue;
    type IntoIter = std::slice::Iter<'a, SlotValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut SlotStorage {
    type Item = &'a mut SlotValue;
    type IntoIter = std::slice::IterMut<'a, SlotValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_slot_payloads_remain_inline() {
        let slots = SlotStorage::from(vec![SlotValue::Scalar(1), SlotValue::Scalar(2)]);

        assert!(!slots.uses_heap_storage());
        assert_eq!(&*slots, &[SlotValue::Scalar(1), SlotValue::Scalar(2)]);
    }
}

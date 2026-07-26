use std::collections::VecDeque;

use super::SEGMENT_LENGTH;

pub(super) type SegmentWindow<Value> = VecDeque<Option<Segment<Value>>>;

#[derive(Clone, Debug)]
pub(super) enum Segment<Value> {
    Dense {
        first: usize,
        values: VecDeque<Value>,
    },
    Sparse(Box<[Option<Value>]>),
}

enum SegmentIter<'a, Value> {
    Dense {
        first: usize,
        values: std::iter::Enumerate<std::collections::vec_deque::Iter<'a, Value>>,
    },
    Sparse(std::iter::Enumerate<std::slice::Iter<'a, Option<Value>>>),
}

enum SegmentIterMut<'a, Value> {
    Dense {
        first: usize,
        values: std::iter::Enumerate<std::collections::vec_deque::IterMut<'a, Value>>,
    },
    Sparse(std::iter::Enumerate<std::slice::IterMut<'a, Option<Value>>>),
}

impl<Value> Segment<Value> {
    pub(super) fn new(offset: usize, value: Value) -> Self {
        let mut values = VecDeque::with_capacity(SEGMENT_LENGTH);
        values.push_back(value);
        Self::Dense {
            first: offset,
            values,
        }
    }

    pub(super) fn get(&self, offset: usize) -> Option<&Value> {
        match self {
            Self::Dense { first, values } => offset
                .checked_sub(*first)
                .and_then(|index| values.get(index)),
            Self::Sparse(entries) => entries.get(offset)?.as_ref(),
        }
    }

    pub(super) fn get_mut(&mut self, offset: usize) -> Option<&mut Value> {
        match self {
            Self::Dense { first, values } => offset
                .checked_sub(*first)
                .and_then(|index| values.get_mut(index)),
            Self::Sparse(entries) => entries.get_mut(offset)?.as_mut(),
        }
    }

    pub(super) fn contains_range(&self, start: usize, end: usize) -> bool {
        match self {
            Self::Dense { first, values } => {
                start >= *first && end <= first.saturating_add(values.len())
            }
            Self::Sparse(entries) => entries[start..end].iter().all(Option::is_some),
        }
    }

    pub(super) fn insert(&mut self, offset: usize, value: Value) -> Option<Value> {
        match self {
            Self::Dense { first, values } if offset == first.saturating_add(values.len()) => {
                values.push_back(value);
                None
            }
            Self::Dense { first, values } if offset.saturating_add(1) == *first => {
                values.push_front(value);
                *first = offset;
                None
            }
            Self::Dense { first, values }
                if (*first..first.saturating_add(values.len())).contains(&offset) =>
            {
                Some(std::mem::replace(&mut values[offset - *first], value))
            }
            Self::Dense { .. } => {
                self.make_sparse();
                self.insert(offset, value)
            }
            Self::Sparse(entries) => entries[offset].replace(value),
        }
    }

    pub(super) fn remove(&mut self, offset: usize) -> Option<Value> {
        match self {
            Self::Dense { first, values } if offset == *first => {
                let value = values.pop_front();
                *first = first.saturating_add(1);
                value
            }
            Self::Dense { first, values }
                if offset.checked_sub(*first) == Some(values.len().saturating_sub(1)) =>
            {
                values.pop_back()
            }
            Self::Dense { first, values }
                if (*first..first.saturating_add(values.len())).contains(&offset) =>
            {
                self.make_sparse();
                self.remove(offset)
            }
            Self::Dense { .. } => None,
            Self::Sparse(entries) => entries[offset].take(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        match self {
            Self::Dense { values, .. } => values.is_empty(),
            Self::Sparse(entries) => entries.iter().all(Option::is_none),
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &Value)> {
        match self {
            Self::Dense { first, values } => SegmentIter::Dense {
                first: *first,
                values: values.iter().enumerate(),
            },
            Self::Sparse(entries) => SegmentIter::Sparse(entries.iter().enumerate()),
        }
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = (usize, &mut Value)> {
        match self {
            Self::Dense { first, values } => SegmentIterMut::Dense {
                first: *first,
                values: values.iter_mut().enumerate(),
            },
            Self::Sparse(entries) => SegmentIterMut::Sparse(entries.iter_mut().enumerate()),
        }
    }

    pub(super) fn next_at(&self, offset: usize) -> Option<(usize, &Value)> {
        self.iter().find(|(candidate, _)| *candidate >= offset)
    }

    fn make_sparse(&mut self) {
        let previous = std::mem::replace(
            self,
            Self::Dense {
                first: 0,
                values: VecDeque::new(),
            },
        );
        let Self::Dense { first, values } = previous else {
            *self = previous;
            return;
        };
        let mut entries = empty_sparse_segment();
        for (relative, value) in values.into_iter().enumerate() {
            entries[first + relative] = Some(value);
        }
        *self = Self::Sparse(entries);
    }
}

impl<'a, Value> Iterator for SegmentIter<'a, Value> {
    type Item = (usize, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Dense { first, values } => values
                .next()
                .map(|(relative, value)| (first.saturating_add(relative), value)),
            Self::Sparse(entries) => {
                entries.find_map(|(offset, entry)| entry.as_ref().map(|value| (offset, value)))
            }
        }
    }
}

impl<'a, Value> Iterator for SegmentIterMut<'a, Value> {
    type Item = (usize, &'a mut Value);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Dense { first, values } => values
                .next()
                .map(|(relative, value)| (first.saturating_add(relative), value)),
            Self::Sparse(entries) => {
                entries.find_map(|(offset, entry)| entry.as_mut().map(|value| (offset, value)))
            }
        }
    }
}

fn empty_sparse_segment<Value>() -> Box<[Option<Value>]> {
    std::iter::repeat_with(|| None)
        .take(SEGMENT_LENGTH)
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

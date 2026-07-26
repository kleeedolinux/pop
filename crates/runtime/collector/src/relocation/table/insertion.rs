use pop_runtime_interface::ManagedReference;

use super::{ObjectTable, Segment, coordinates};

impl<Value> ObjectTable<Value> {
    pub(crate) fn insert(&mut self, reference: ManagedReference, value: Value) -> Option<Value> {
        let (segment, offset) = coordinates(reference).expect("managed references are nonzero");
        let index = self.ensure_segment_index(segment);
        let previous = match &mut self.segments[index] {
            Some(entries) => entries.insert(offset, value),
            slot @ None => {
                *slot = Some(Segment::new(offset, value));
                None
            }
        };
        if previous.is_none() {
            self.length = self.length.saturating_add(1);
        }
        self.highest_reference = Some(
            self.highest_reference
                .map_or(reference.raw(), |highest| highest.max(reference.raw())),
        );
        previous
    }

    /// Inserts a token that has never previously appeared in this table.
    ///
    /// The fresh-token path preserves the general table's sparse layout while
    /// avoiding its bidirectional window maintenance on monotonic allocation.
    pub(crate) fn insert_fresh(
        &mut self,
        reference: ManagedReference,
        value: Value,
    ) -> Result<(), Value> {
        if self
            .highest_reference
            .is_some_and(|highest| reference.raw() <= highest)
        {
            return Err(value);
        }
        let (segment, offset) = coordinates(reference).expect("managed references are nonzero");
        match self.base_segment {
            None => {
                self.base_segment = Some(segment);
                self.segments.push_back(None);
            }
            Some(base) => {
                let index =
                    usize::try_from(segment - base).expect("fresh token segment fits usize");
                if self.segments.len() <= index {
                    self.segments.resize_with(index + 1, || None);
                }
            }
        }
        let slot = self
            .segments
            .back_mut()
            .expect("fresh token creates a tail segment");
        match slot {
            Some(entries) => {
                if let Some(previous) = entries.insert(offset, value) {
                    return Err(previous);
                }
            }
            slot @ None => *slot = Some(Segment::new(offset, value)),
        }
        self.length = self.length.saturating_add(1);
        self.highest_reference = Some(reference.raw());
        Ok(())
    }

    pub(crate) fn insert_reserved(
        &mut self,
        reference: ManagedReference,
        value: Value,
    ) -> Result<(), Value> {
        if self
            .highest_reference
            .is_none_or(|highest| reference.raw() > highest)
        {
            return self.insert_fresh(reference, value);
        }
        if self.contains_key(&reference) {
            return Err(value);
        }
        let previous = self.insert(reference, value);
        debug_assert!(previous.is_none());
        Ok(())
    }

    pub(crate) fn insert_reserved_batch<I>(&mut self, entries: I) -> Result<(), ()>
    where
        I: IntoIterator<Item = (ManagedReference, Value)>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut entries = entries.into_iter();
        let length = entries.len();
        let Some((first_reference, first_value)) = entries.next() else {
            return Ok(());
        };
        if self
            .highest_reference
            .is_some_and(|highest| first_reference.raw() <= highest)
        {
            self.insert_reserved(first_reference, first_value)
                .map_err(|_| ())?;
            for (reference, value) in entries {
                self.insert_reserved(reference, value).map_err(|_| ())?;
            }
            return Ok(());
        }

        let last_raw = first_reference
            .raw()
            .checked_add(u64::try_from(length.saturating_sub(1)).map_err(|_| ())?)
            .ok_or(())?;
        let (first_segment, _) = coordinates(first_reference).ok_or(())?;
        let (last_segment, _) = coordinates(ManagedReference::new(last_raw)).ok_or(())?;
        match self.base_segment {
            None => {
                self.base_segment = Some(first_segment);
                self.segments.resize_with(
                    usize::try_from(last_segment - first_segment + 1).map_err(|_| ())?,
                    || None,
                );
            }
            Some(base) => {
                let last_index = usize::try_from(last_segment - base).map_err(|_| ())?;
                if self.segments.len() <= last_index {
                    self.segments.resize_with(last_index + 1, || None);
                }
            }
        }
        let mut insert = |reference: ManagedReference, value: Value| {
            let (segment, offset) =
                coordinates(reference).expect("validated fresh batch reference");
            let index = self
                .segment_index(segment)
                .expect("fresh batch segment was reserved");
            let slot = &mut self.segments[index];
            match slot {
                Some(entries) => {
                    let previous = entries.insert(offset, value);
                    debug_assert!(previous.is_none());
                }
                slot @ None => *slot = Some(Segment::new(offset, value)),
            }
        };
        insert(first_reference, first_value);
        for (offset, (reference, value)) in entries.enumerate() {
            debug_assert_eq!(
                reference.raw(),
                first_reference.raw() + u64::try_from(offset + 1).unwrap_or(u64::MAX)
            );
            insert(reference, value);
        }
        self.length = self.length.saturating_add(length);
        self.highest_reference = Some(last_raw);
        Ok(())
    }
}

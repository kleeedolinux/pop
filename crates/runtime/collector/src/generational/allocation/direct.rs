//! Checked retained page access and direct-store capabilities.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use pop_runtime_interface::{ManagedReference, ObjectMap, ObjectSlot, SchedulerId};

use crate::heap::PageWords;

pub struct DirectPageAccess {
    first_reference: u64,
    published_last_reference: Arc<AtomicU64>,
    first_word: usize,
    words_per_object: usize,
    payload: PageWords,
    object_map: Arc<ObjectMap>,
    state: Arc<DirectAccessState>,
    captured_revision: u64,
    writer_active: OnceLock<Arc<AtomicBool>>,
    thread_bound: PhantomData<Cell<()>>,
}

#[derive(Debug, Default)]
pub(crate) struct DirectAccessState {
    revision: AtomicU64,
    writers: Mutex<Vec<Weak<AtomicBool>>>,
}

struct DirectWriteGuard<'a> {
    active: &'a AtomicBool,
}

pub struct DirectReferenceStoreAccess {
    access: DirectPageAccess,
    scheduler: SchedulerId,
}

pub struct DirectReferenceValidation {
    kind: DirectReferenceValidationKind,
    scheduler: SchedulerId,
}

pub(crate) struct DirectReferenceLease {
    first_reference: ManagedReference,
    last_reference: ManagedReference,
    published_last_reference: Rc<Cell<u64>>,
    state: Arc<DirectAccessState>,
    captured_revision: u64,
    scheduler: SchedulerId,
}

enum DirectReferenceValidationKind {
    Span(Box<DirectPageAccess>),
    Lease {
        first_reference: ManagedReference,
        last_reference: ManagedReference,
        published_last_reference: Rc<Cell<u64>>,
        state: Arc<DirectAccessState>,
        captured_revision: u64,
    },
}

impl DirectAccessState {
    fn capture(&self) -> Option<u64> {
        let revision = self.revision.load(Ordering::Acquire);
        revision.is_multiple_of(2).then_some(revision)
    }

    fn register_writer(&self, captured_revision: u64) -> Option<Arc<AtomicBool>> {
        if self.revision.load(Ordering::Acquire) != captured_revision {
            return None;
        }
        let active = Arc::new(AtomicBool::new(false));
        self.writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::downgrade(&active));
        (self.revision.load(Ordering::Acquire) == captured_revision).then_some(active)
    }

    fn begin_write<'a>(
        &self,
        captured_revision: u64,
        active: &'a AtomicBool,
    ) -> Option<DirectWriteGuard<'a>> {
        if !captured_revision.is_multiple_of(2)
            || self.revision.load(Ordering::Acquire) != captured_revision
        {
            return None;
        }
        active.store(true, Ordering::Release);
        if self.revision.load(Ordering::Acquire) == captured_revision {
            Some(DirectWriteGuard { active })
        } else {
            active.store(false, Ordering::Release);
            None
        }
    }

    #[inline]
    fn begin_buffered_write(&self, captured_revision: u64, active: &AtomicBool) -> bool {
        if active.load(Ordering::Relaxed) {
            return self.revision.load(Ordering::Acquire) == captured_revision;
        }
        if self.revision.load(Ordering::Acquire) != captured_revision {
            return false;
        }
        active.store(true, Ordering::Release);
        if self.revision.load(Ordering::Acquire) == captured_revision {
            true
        } else {
            active.store(false, Ordering::Release);
            false
        }
    }

    pub(super) fn invalidate(&self) {
        let previous = self.revision.fetch_add(1, Ordering::AcqRel);
        debug_assert!(previous.is_multiple_of(2));
        loop {
            let active = {
                let mut writers = self
                    .writers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                writers.retain(|writer| writer.strong_count() != 0);
                writers
                    .iter()
                    .filter_map(Weak::upgrade)
                    .any(|writer| writer.load(Ordering::Acquire))
            };
            if !active {
                break;
            }
            std::thread::yield_now();
        }
        self.revision.fetch_add(1, Ordering::Release);
    }

    #[inline]
    fn still_valid(&self, captured_revision: u64) -> bool {
        self.revision.load(Ordering::Acquire) == captured_revision
    }
}

impl Drop for DirectWriteGuard<'_> {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

impl DirectPageAccess {
    pub(super) fn new(
        first_reference: ManagedReference,
        published_last_reference: Arc<AtomicU64>,
        first_word: usize,
        words_per_object: usize,
        payload: PageWords,
        object_map: Arc<ObjectMap>,
        state: Arc<DirectAccessState>,
    ) -> Option<Self> {
        let last_reference = published_last_reference.load(Ordering::Acquire);
        let last_relative =
            usize::try_from(last_reference.checked_sub(first_reference.raw())?).ok()?;
        let last_word = first_word
            .checked_add(last_relative.checked_mul(words_per_object)?)?
            .checked_add(words_per_object.checked_sub(1)?)?;
        if last_word >= payload.len() {
            return None;
        }
        let captured_revision = state.capture()?;
        Some(Self {
            first_reference: first_reference.raw(),
            published_last_reference,
            first_word,
            words_per_object,
            payload,
            object_map,
            state,
            captured_revision,
            writer_active: OnceLock::new(),
            thread_bound: PhantomData,
        })
    }

    #[must_use]
    #[inline]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        (self.first_reference..=self.published_last_reference.load(Ordering::Acquire))
            .contains(&reference.raw())
    }

    #[must_use]
    #[inline]
    pub fn load(&self, reference: ManagedReference, slot: ObjectSlot) -> Option<u64> {
        let index = self.word_index(reference, slot)?;
        let value = self.payload.get(index)?.load(Ordering::Relaxed);
        // The retained page keeps the physical word alive. The final revision
        // check is the read's linearization point and rejects every placement
        // invalidation that precedes publication of the result.
        self.state
            .still_valid(self.captured_revision)
            .then_some(value)
    }

    /// Reports whether a valid retained slot is scalar.
    #[must_use]
    pub fn slot_is_scalar(&self, reference: ManagedReference, slot: ObjectSlot) -> Option<bool> {
        self.word_index(reference, slot)?;
        self.state
            .still_valid(self.captured_revision)
            .then(|| !self.object_map.is_reference_slot(slot))
    }

    /// Stores one scalar word while the retained placement remains valid.
    ///
    /// Returns `false` for a stale access, an out-of-range slot, or a
    /// reference-designated slot. Relocation invalidation waits for an admitted
    /// direct writer before copying or replacing the page.
    #[must_use]
    pub fn store_scalar(&self, reference: ManagedReference, slot: ObjectSlot, value: u64) -> bool {
        if self.slot_is_scalar(reference, slot) != Some(true) {
            return false;
        }
        let Some(writer_active) = self.writer_active() else {
            return false;
        };
        let Some(_writer) = self
            .state
            .begin_write(self.captured_revision, writer_active)
        else {
            return false;
        };
        self.store_word(reference, slot, value)
    }

    fn store_reference(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<(ManagedReference, &DirectReferenceValidation)>,
    ) -> bool {
        if self.word_index(owner, slot).is_none() || !self.object_map.is_reference_slot(slot) {
            return false;
        }
        let Some(writer_active) = self.writer_active() else {
            return false;
        };
        let Some(_writer) = self
            .state
            .begin_write(self.captured_revision, writer_active)
        else {
            return false;
        };
        let raw = match value {
            None => 0,
            Some((reference, validation))
                if validation.matches(&self.state, self.captured_revision, reference) =>
            {
                reference.raw()
            }
            Some(_) => return false,
        };
        self.store_word(owner, slot, raw)
    }

    #[inline]
    fn store_reference_buffered(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<(ManagedReference, &DirectReferenceValidation)>,
    ) -> bool {
        let Some(index) = self.word_index(owner, slot) else {
            return false;
        };
        if !self.object_map.is_reference_slot(slot) {
            return false;
        }
        let Some(writer_active) = self.writer_active() else {
            return false;
        };
        if !self
            .state
            .begin_buffered_write(self.captured_revision, writer_active)
        {
            return false;
        }
        let raw = match value {
            None => 0,
            Some((reference, validation))
                if validation.matches_buffered(&self.state, self.captured_revision, reference) =>
            {
                reference.raw()
            }
            Some(_) => return false,
        };
        let Some(word) = self.payload.get(index) else {
            return false;
        };
        word.store(raw, Ordering::Relaxed);
        true
    }

    #[inline]
    fn word_index(&self, reference: ManagedReference, slot: ObjectSlot) -> Option<usize> {
        if !self.contains(reference) || slot.raw() as usize >= self.words_per_object {
            return None;
        }
        let relative = usize::try_from(reference.raw() - self.first_reference).ok()?;
        self.first_word
            .checked_add(relative.checked_mul(self.words_per_object)?)?
            .checked_add(slot.raw() as usize)
    }

    fn store_word(&self, reference: ManagedReference, slot: ObjectSlot, value: u64) -> bool {
        let Some(index) = self.word_index(reference, slot) else {
            return false;
        };
        let Some(word) = self.payload.get(index) else {
            return false;
        };
        word.store(value, Ordering::Relaxed);
        true
    }

    #[inline]
    fn writer_active(&self) -> Option<&AtomicBool> {
        if let Some(active) = self.writer_active.get() {
            return Some(active);
        }
        let active = self.state.register_writer(self.captured_revision)?;
        let _ = self.writer_active.set(active);
        self.writer_active.get().map(Arc::as_ref)
    }

    fn prepare_writer(&self) -> bool {
        self.writer_active().is_some()
    }

    fn quiesce_writer(&self) {
        if let Some(active) = self.writer_active.get() {
            active.store(false, Ordering::Release);
        }
    }
}

impl Drop for DirectPageAccess {
    fn drop(&mut self) {
        self.quiesce_writer();
    }
}

impl DirectReferenceStoreAccess {
    pub(crate) fn new(access: DirectPageAccess, scheduler: SchedulerId) -> Option<Self> {
        access
            .prepare_writer()
            .then_some(Self { access, scheduler })
    }

    /// Reports whether this retained access covers the owner token.
    #[must_use]
    #[inline]
    pub fn contains(&self, owner: ManagedReference) -> bool {
        self.access.contains(owner)
    }

    /// Stores a scheduler-local mature edge without SATB or card work.
    ///
    /// `value` must carry a validation capability issued for the same
    /// scheduler and direct-access revision. `None` stores a null reference.
    #[must_use]
    pub fn store(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<(ManagedReference, &DirectReferenceValidation)>,
    ) -> bool {
        let value = match value {
            None => None,
            Some((reference, validation)) if validation.scheduler == self.scheduler => {
                Some((reference, validation))
            }
            Some(_) => return false,
        };
        self.access.store_reference(owner, slot, value)
    }

    /// Buffers writer admission across scheduler-local stores until quiesced.
    #[must_use]
    #[inline]
    pub fn store_buffered(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<(ManagedReference, &DirectReferenceValidation)>,
    ) -> bool {
        let value = match value {
            None => None,
            Some((reference, validation)) if validation.scheduler == self.scheduler => {
                Some((reference, validation))
            }
            Some(_) => return false,
        };
        self.access.store_reference_buffered(owner, slot, value)
    }

    pub fn quiesce(&self) {
        self.access.quiesce_writer();
    }
}

impl DirectReferenceValidation {
    pub(crate) fn new(access: DirectPageAccess, scheduler: SchedulerId) -> Self {
        Self {
            kind: DirectReferenceValidationKind::Span(Box::new(access)),
            scheduler,
        }
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        match &self.kind {
            DirectReferenceValidationKind::Span(access) => {
                access.contains(reference) && access.state.still_valid(access.captured_revision)
            }
            DirectReferenceValidationKind::Lease {
                first_reference,
                last_reference,
                published_last_reference,
                state,
                captured_revision,
            } => {
                (*first_reference..=*last_reference).contains(&reference)
                    && reference.raw() <= published_last_reference.get()
                    && state.still_valid(*captured_revision)
            }
        }
    }

    fn matches(
        &self,
        owner_state: &Arc<DirectAccessState>,
        owner_revision: u64,
        reference: ManagedReference,
    ) -> bool {
        match &self.kind {
            DirectReferenceValidationKind::Span(access) => {
                Arc::ptr_eq(owner_state, &access.state)
                    && owner_revision == access.captured_revision
                    && access.contains(reference)
                    && access.state.still_valid(access.captured_revision)
            }
            DirectReferenceValidationKind::Lease {
                first_reference,
                last_reference,
                published_last_reference,
                state,
                captured_revision,
            } => {
                (*first_reference..=*last_reference).contains(&reference)
                    && reference.raw() <= published_last_reference.get()
                    && Arc::ptr_eq(owner_state, state)
                    && owner_revision == *captured_revision
                    && state.still_valid(*captured_revision)
            }
        }
    }

    #[inline]
    fn matches_buffered(
        &self,
        owner_state: &Arc<DirectAccessState>,
        owner_revision: u64,
        reference: ManagedReference,
    ) -> bool {
        match &self.kind {
            DirectReferenceValidationKind::Span(access) => {
                Arc::ptr_eq(owner_state, &access.state)
                    && owner_revision == access.captured_revision
                    && access.contains(reference)
            }
            DirectReferenceValidationKind::Lease {
                first_reference,
                last_reference,
                published_last_reference,
                state,
                captured_revision,
            } => {
                (*first_reference..=*last_reference).contains(&reference)
                    && reference.raw() <= published_last_reference.get()
                    && Arc::ptr_eq(owner_state, state)
                    && owner_revision == *captured_revision
            }
        }
    }
}

impl DirectReferenceLease {
    pub(crate) fn new(
        first_reference: ManagedReference,
        last_reference: ManagedReference,
        state: Arc<DirectAccessState>,
        scheduler: SchedulerId,
    ) -> Option<Self> {
        let captured_revision = state.capture()?;
        Some(Self {
            first_reference,
            last_reference,
            published_last_reference: Rc::new(Cell::new(first_reference.raw().saturating_sub(1))),
            state,
            captured_revision,
            scheduler,
        })
    }

    #[inline]
    pub(crate) fn complete(&self, reference: ManagedReference) -> bool {
        let expected = self.published_last_reference.get().saturating_add(1);
        if reference.raw() != expected
            || reference.raw() > self.last_reference.raw()
            || !self.state.still_valid(self.captured_revision)
        {
            return false;
        }
        self.published_last_reference.set(reference.raw());
        true
    }

    pub(crate) fn validation(&self) -> DirectReferenceValidation {
        DirectReferenceValidation {
            kind: DirectReferenceValidationKind::Lease {
                first_reference: self.first_reference,
                last_reference: self.last_reference,
                published_last_reference: self.published_last_reference.clone(),
                state: self.state.clone(),
                captured_revision: self.captured_revision,
            },
            scheduler: self.scheduler,
        }
    }

    pub(crate) fn still_valid(&self) -> bool {
        self.state.still_valid(self.captured_revision)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::DirectAccessState;

    #[test]
    fn invalidation_waits_for_an_admitted_direct_writer() {
        let state = Arc::new(DirectAccessState::default());
        let captured = state.capture().expect("stable revision");
        let active = state.register_writer(captured).expect("writer slot");
        let writer = state.begin_write(captured, &active).expect("direct writer");
        let invalidating = state.clone();
        let handle = std::thread::spawn(move || invalidating.invalidate());

        while state.revision.load(Ordering::Acquire).is_multiple_of(2) {
            std::thread::yield_now();
        }
        assert!(!handle.is_finished());
        drop(writer);
        handle.join().expect("invalidation");
        assert_ne!(state.capture(), Some(captured));
    }

    #[test]
    fn invalidation_waits_until_a_buffered_writer_quiesces() {
        let state = Arc::new(DirectAccessState::default());
        let captured = state.capture().expect("stable revision");
        let active = state.register_writer(captured).expect("writer slot");
        assert!(state.begin_buffered_write(captured, &active));
        let invalidating = state.clone();
        let handle = std::thread::spawn(move || invalidating.invalidate());

        while state.revision.load(Ordering::Acquire).is_multiple_of(2) {
            std::thread::yield_now();
        }
        assert!(!handle.is_finished());
        active.store(false, Ordering::Release);
        handle.join().expect("invalidation");
        assert_ne!(state.capture(), Some(captured));
    }
}

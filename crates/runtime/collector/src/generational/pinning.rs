//! Deterministic scoped-pin accounting and safe-point-age telemetry.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::{ManagedReference, PinHandle, RuntimeFailure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinningConfig {
    long_lived_pin_safe_points: u64,
}

impl PinningConfig {
    #[must_use]
    pub const fn new(long_lived_pin_safe_points: u64) -> Self {
        Self {
            long_lived_pin_safe_points: if long_lived_pin_safe_points == 0 {
                1
            } else {
                long_lived_pin_safe_points
            },
        }
    }

    #[must_use]
    pub const fn long_lived_pin_safe_points(self) -> u64 {
        self.long_lived_pin_safe_points
    }
}

impl Default for PinningConfig {
    fn default() -> Self {
        Self::new(1_024)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PinningTelemetry {
    pins_created: u64,
    pins_released: u64,
    active_pin_handles: usize,
    pinned_objects: usize,
    safe_points_observed: u64,
    current_maximum_pin_age_safe_points: u64,
    maximum_completed_pin_age_safe_points: u64,
    long_lived_pins_reported: u64,
}

macro_rules! telemetry_accessors {
    ($($name:ident: $type:ty),* $(,)?) => {
        $(
            #[must_use]
            pub const fn $name(self) -> $type {
                self.$name
            }
        )*
    };
}

impl PinningTelemetry {
    telemetry_accessors! {
        pins_created: u64,
        pins_released: u64,
        active_pin_handles: usize,
        pinned_objects: usize,
        safe_points_observed: u64,
        current_maximum_pin_age_safe_points: u64,
        maximum_completed_pin_age_safe_points: u64,
        long_lived_pins_reported: u64,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PinRecord {
    reference: ManagedReference,
    started_at_safe_point: u64,
    long_lived_reported: bool,
}

pub(crate) struct PinningState {
    config: PinningConfig,
    safe_points_observed: u64,
    records: BTreeMap<PinHandle, PinRecord>,
    pins_created: u64,
    pins_released: u64,
    maximum_completed_pin_age_safe_points: u64,
    long_lived_pins_reported: u64,
}

impl PinningState {
    pub(crate) fn new(config: PinningConfig) -> Self {
        Self {
            config,
            safe_points_observed: 0,
            records: BTreeMap::new(),
            pins_created: 0,
            pins_released: 0,
            maximum_completed_pin_age_safe_points: 0,
            long_lived_pins_reported: 0,
        }
    }

    pub(crate) fn register(
        &mut self,
        pin: PinHandle,
        reference: ManagedReference,
    ) -> Result<(), RuntimeFailure> {
        if self
            .records
            .insert(
                pin,
                PinRecord {
                    reference,
                    started_at_safe_point: self.safe_points_observed,
                    long_lived_reported: false,
                },
            )
            .is_some()
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.pins_created = self.pins_created.saturating_add(1);
        Ok(())
    }

    pub(crate) fn record(&self, pin: PinHandle) -> Option<PinRecord> {
        self.records.get(&pin).copied()
    }

    pub(crate) fn complete_unpin(
        &mut self,
        pin: PinHandle,
        expected: PinRecord,
    ) -> Result<(), RuntimeFailure> {
        let removed = self
            .records
            .remove(&pin)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if removed != expected {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.pins_released = self.pins_released.saturating_add(1);
        self.maximum_completed_pin_age_safe_points =
            self.maximum_completed_pin_age_safe_points.max(
                self.safe_points_observed
                    .saturating_sub(removed.started_at_safe_point),
            );
        Ok(())
    }

    pub(crate) fn advance_safe_point(&mut self) {
        self.safe_points_observed = self.safe_points_observed.saturating_add(1);
        for record in self.records.values_mut() {
            let age = self
                .safe_points_observed
                .saturating_sub(record.started_at_safe_point);
            if !record.long_lived_reported && age >= self.config.long_lived_pin_safe_points {
                record.long_lived_reported = true;
                self.long_lived_pins_reported = self.long_lived_pins_reported.saturating_add(1);
            }
        }
    }

    pub(crate) fn telemetry(&self) -> PinningTelemetry {
        let pinned_objects = self
            .records
            .values()
            .map(|record| record.reference)
            .collect::<BTreeSet<_>>()
            .len();
        let current_maximum_pin_age_safe_points = self
            .records
            .values()
            .map(|record| {
                self.safe_points_observed
                    .saturating_sub(record.started_at_safe_point)
            })
            .max()
            .unwrap_or(0);
        PinningTelemetry {
            pins_created: self.pins_created,
            pins_released: self.pins_released,
            active_pin_handles: self.records.len(),
            pinned_objects,
            safe_points_observed: self.safe_points_observed,
            current_maximum_pin_age_safe_points,
            maximum_completed_pin_age_safe_points: self.maximum_completed_pin_age_safe_points,
            long_lived_pins_reported: self.long_lived_pins_reported,
        }
    }
}

//! Generational card refinement and precise reference write barriers.

use std::collections::BTreeMap;

use pop_runtime_interface::{ManagedReference, ObjectSlot, RuntimeFailure};

use super::{CollectorGeneration, RelocationRuntime};

impl RelocationRuntime {
    pub(crate) fn install_refined_cards(
        &mut self,
        refined: BTreeMap<ManagedReference, Vec<ManagedReference>>,
    ) -> Result<(), RuntimeFailure> {
        if refined
            .keys()
            .any(|owner| !self.dirty_cards.contains(owner))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        for (owner, children) in &refined {
            if self.generation(*owner) != Some(CollectorGeneration::Mature)
                || children.iter().any(|child| {
                    !matches!(
                        self.generation(*child),
                        Some(CollectorGeneration::Nursery { .. })
                    )
                })
            {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        self.refined_cards = Some(refined);
        Ok(())
    }

    pub(super) fn apply_reference_barrier(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        previous: Option<ManagedReference>,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        if self.load_reference(owner, slot)? != previous {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let owner_is_mature = self.generation(owner) == Some(CollectorGeneration::Mature);
        let value_is_young = value.is_some_and(|reference| {
            matches!(
                self.generation(reference),
                Some(CollectorGeneration::Nursery { .. })
            )
        });
        if owner_is_mature && value_is_young {
            self.dirty_cards.insert(owner);
        }
        Ok(())
    }
}

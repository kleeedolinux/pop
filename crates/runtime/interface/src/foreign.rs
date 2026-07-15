//! Backend-neutral foreign-call transition identities and modes.

use core::num::NonZeroU64;

/// The collector participation contract selected for one statically resolved
/// native call.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ForeignCallMode {
    Blocking = 0,
    BoundedNonblocking = 1,
}

impl ForeignCallMode {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::Blocking,
            1 => Self::BoundedNonblocking,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn raw(self) -> u8 {
        self as u8
    }
}

/// An opaque, thread-bound identity for one balanced foreign transition.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ForeignTransitionId(NonZeroU64);

impl ForeignTransitionId {
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        match NonZeroU64::new(raw) {
            Some(raw) => Some(Self(raw)),
            None => None,
        }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0.get()
    }
}

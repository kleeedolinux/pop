use crate::{ManagedReference, RuntimeFailure, SchedulerId};

pub const IMMUTABLE_BYTES_RUNTIME_TYPE_ID: crate::RuntimeTypeId = crate::RuntimeTypeId::new(2);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FfiAbiLayoutId(u64);

impl FfiAbiLayoutId {
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FfiBufferBorrowId(u64);

impl FfiBufferBorrowId {
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FfiBytesBorrowId(u64);

impl FfiBytesBorrowId {
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ForeignAddress(u64);

impl ForeignAddress {
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

macro_rules! nonzero_callback_identity {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(core::num::NonZeroU64);

        impl $name {
            #[must_use]
            pub const fn new(raw: u64) -> Option<Self> {
                match core::num::NonZeroU64::new(raw) {
                    Some(raw) => Some(Self(raw)),
                    None => None,
                }
            }

            #[must_use]
            pub const fn raw(self) -> u64 {
                self.0.get()
            }
        }
    };
}

nonzero_callback_identity!(FfiCallbackSiteId);
nonzero_callback_identity!(FfiCallbackRegistrationId);
nonzero_callback_identity!(FfiCallbackTransitionId);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum FfiCallbackLifetime {
    CallScoped = 0,
    Registered = 1,
}

impl FfiCallbackLifetime {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::CallScoped,
            1 => Self::Registered,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn raw(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum FfiCallbackThread {
    CallingThread = 0,
    AttachedThread = 1,
}

impl FfiCallbackThread {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::CallingThread,
            1 => Self::AttachedThread,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn raw(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfiCallbackOpenRequest {
    environment: Option<ManagedReference>,
    site: FfiCallbackSiteId,
    scheduler: SchedulerId,
    lifetime: FfiCallbackLifetime,
    thread: FfiCallbackThread,
}

impl FfiCallbackOpenRequest {
    #[must_use]
    pub const fn new(
        environment: Option<ManagedReference>,
        site: FfiCallbackSiteId,
        scheduler: SchedulerId,
        lifetime: FfiCallbackLifetime,
        thread: FfiCallbackThread,
    ) -> Self {
        Self {
            environment,
            site,
            scheduler,
            lifetime,
            thread,
        }
    }

    #[must_use]
    pub const fn environment(self) -> Option<ManagedReference> {
        self.environment
    }

    #[must_use]
    pub const fn site(self) -> FfiCallbackSiteId {
        self.site
    }

    #[must_use]
    pub const fn scheduler(self) -> SchedulerId {
        self.scheduler
    }

    #[must_use]
    pub const fn lifetime(self) -> FfiCallbackLifetime {
        self.lifetime
    }

    #[must_use]
    pub const fn thread(self) -> FfiCallbackThread {
        self.thread
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfiCallbackRegistration {
    id: FfiCallbackRegistrationId,
    context: ForeignAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FfiCallbackOpenFailure {
    Allocation,
    Invariant(RuntimeFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FfiCallbackCloseFailure {
    InUse,
    Invariant(RuntimeFailure),
}

impl FfiCallbackRegistration {
    #[must_use]
    pub const fn new(id: FfiCallbackRegistrationId, context: ForeignAddress) -> Self {
        Self { id, context }
    }

    #[must_use]
    pub const fn id(self) -> FfiCallbackRegistrationId {
        self.id
    }

    #[must_use]
    pub const fn context(self) -> ForeignAddress {
        self.context
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfiCallbackEntry {
    transition: FfiCallbackTransitionId,
    environment: Option<ManagedReference>,
}

impl FfiCallbackEntry {
    #[must_use]
    pub const fn new(
        transition: FfiCallbackTransitionId,
        environment: Option<ManagedReference>,
    ) -> Self {
        Self {
            transition,
            environment,
        }
    }

    #[must_use]
    pub const fn transition(self) -> FfiCallbackTransitionId {
        self.transition
    }

    #[must_use]
    pub const fn environment(self) -> Option<ManagedReference> {
        self.environment
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfiBufferOpenRequest {
    length: u64,
    element_size: u64,
    alignment: u64,
    layout: FfiAbiLayoutId,
}

impl FfiBufferOpenRequest {
    /// Creates checked foreign-storage geometry for one exact ABI layout.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for zero element size, invalid alignment,
    /// or byte-length overflow.
    pub fn new(
        length: u64,
        element_size: u64,
        alignment: u64,
        layout: FfiAbiLayoutId,
    ) -> Result<Self, RuntimeFailure> {
        if element_size == 0
            || alignment == 0
            || !alignment.is_power_of_two()
            || length.checked_mul(element_size).is_none()
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        Ok(Self {
            length,
            element_size,
            alignment,
            layout,
        })
    }

    #[must_use]
    pub const fn length(self) -> u64 {
        self.length
    }
    #[must_use]
    pub const fn element_size(self) -> u64 {
        self.element_size
    }
    #[must_use]
    pub const fn alignment(self) -> u64 {
        self.alignment
    }
    #[must_use]
    pub const fn layout(self) -> FfiAbiLayoutId {
        self.layout
    }
    #[must_use]
    pub const fn byte_length(self) -> u64 {
        self.length * self.element_size
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FfiBufferOpenFailure {
    Allocation,
    Invariant(RuntimeFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfiBufferBorrow {
    id: FfiBufferBorrowId,
    address: Option<ForeignAddress>,
    length: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfiBytesBorrow {
    id: FfiBytesBorrowId,
    address: Option<ForeignAddress>,
    length: u64,
}

impl FfiBytesBorrow {
    #[must_use]
    pub const fn new(
        id: FfiBytesBorrowId,
        address: Option<ForeignAddress>,
        length: u64,
    ) -> Option<Self> {
        if (length == 0) != address.is_none() {
            return None;
        }
        Some(Self {
            id,
            address,
            length,
        })
    }

    #[must_use]
    pub const fn id(self) -> FfiBytesBorrowId {
        self.id
    }

    #[must_use]
    pub const fn address(self) -> Option<ForeignAddress> {
        self.address
    }

    #[must_use]
    pub const fn length(self) -> u64 {
        self.length
    }
}

impl FfiBufferBorrow {
    #[must_use]
    pub const fn new(id: FfiBufferBorrowId, address: Option<ForeignAddress>, length: u64) -> Self {
        Self {
            id,
            address,
            length,
        }
    }
    #[must_use]
    pub const fn id(self) -> FfiBufferBorrowId {
        self.id
    }
    #[must_use]
    pub const fn address(self) -> Option<ForeignAddress> {
        self.address
    }
    #[must_use]
    pub const fn length(self) -> u64 {
        self.length
    }
}

pub type FfiBufferReference = ManagedReference;

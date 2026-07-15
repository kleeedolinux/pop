use crate::{ManagedReference, RuntimeFailure};

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

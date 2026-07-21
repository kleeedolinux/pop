#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NativeAbiVersion {
    major: u16,
    minor: u16,
}

impl NativeAbiVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    #[must_use]
    pub const fn major(self) -> u16 {
        self.major
    }

    #[must_use]
    pub const fn minor(self) -> u16 {
        self.minor
    }
}

pub const NATIVE_ABI_1_VERSION: NativeAbiVersion = NativeAbiVersion::new(1, 19);
pub const NATIVE_ABI_2_VERSION: NativeAbiVersion = NativeAbiVersion::new(2, 0);
pub const ABI_SUPPORT_SYMBOL: &str = "pop_rt_supports_abi";
pub const GC_SAFE_POINT_V2_SYMBOL: &str = "pop_rt_gc_safe_point_v2";
pub const INVALID_HANDLE: u64 = 0;

/// Exact ABI 1.19 writer event function shape.
pub type CodecWriteEventAbi = unsafe extern "C" fn(u64, u8, u32, *const u8, u64, u64, u64) -> u8;

/// Exact ABI 1.19 reader event function shape.
pub type CodecReadEventAbi = unsafe extern "C" fn(
    u64,
    *mut u8,
    *mut u32,
    *mut *const u8,
    *mut u64,
    *mut u64,
    *mut u64,
) -> u8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CodecEventStatus {
    Ok = 0,
    MalformedInput = 1,
    LimitExceeded = 2,
    CapabilityFailure = 3,
}

impl CodecEventStatus {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::Ok,
            1 => Self::MalformedInput,
            2 => Self::LimitExceeded,
            3 => Self::CapabilityFailure,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CodecEventTag {
    RecordStart = 0,
    Member = 1,
    RecordEnd = 2,
    EnumCase = 3,
    UnionStart = 4,
    Payload = 5,
    UnionEnd = 6,
    TupleStart = 7,
    Element = 8,
    TupleEnd = 9,
    SequenceStart = 10,
    SequenceEnd = 11,
    OptionalAbsent = 12,
    OptionalPresent = 13,
    Boolean = 14,
    Int8 = 15,
    Int16 = 16,
    Int32 = 17,
    Int64 = 18,
    UInt8 = 19,
    UInt16 = 20,
    UInt32 = 21,
    UInt64 = 22,
    Float32 = 23,
    Float64 = 24,
    String = 25,
    Bytes = 26,
}

impl CodecEventTag {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::RecordStart,
            1 => Self::Member,
            2 => Self::RecordEnd,
            3 => Self::EnumCase,
            4 => Self::UnionStart,
            5 => Self::Payload,
            6 => Self::UnionEnd,
            7 => Self::TupleStart,
            8 => Self::Element,
            9 => Self::TupleEnd,
            10 => Self::SequenceStart,
            11 => Self::SequenceEnd,
            12 => Self::OptionalAbsent,
            13 => Self::OptionalPresent,
            14 => Self::Boolean,
            15 => Self::Int8,
            16 => Self::Int16,
            17 => Self::Int32,
            18 => Self::Int64,
            19 => Self::UInt8,
            20 => Self::UInt16,
            21 => Self::UInt32,
            22 => Self::UInt64,
            23 => Self::Float32,
            24 => Self::Float64,
            25 => Self::String,
            26 => Self::Bytes,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NativeTaskStatus {
    Failure = 0,
    Ready = 1,
    Pending = 2,
    Completed = 3,
    Cancelled = 4,
    Panicked = 5,
}

impl NativeTaskStatus {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::Failure,
            1 => Self::Ready,
            2 => Self::Pending,
            3 => Self::Completed,
            4 => Self::Cancelled,
            5 => Self::Panicked,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IterationCollectionKind {
    Array = 0,
    Table = 1,
    List = 2,
    Range = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IterationStatus {
    Failure = 0,
    Item = 1,
    End = 2,
    ConcurrentModification = 3,
    IntegerOverflow = 4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum StringFormatTag {
    Boolean = 0,
    Int8 = 1,
    Int16 = 2,
    Int32 = 3,
    Int64 = 4,
    UInt8 = 5,
    UInt16 = 6,
    UInt32 = 7,
    UInt64 = 8,
    Float32 = 9,
    Float64 = 10,
}

impl StringFormatTag {
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Boolean,
            1 => Self::Int8,
            2 => Self::Int16,
            3 => Self::Int32,
            4 => Self::Int64,
            5 => Self::UInt8,
            6 => Self::UInt16,
            7 => Self::UInt32,
            8 => Self::UInt64,
            9 => Self::Float32,
            10 => Self::Float64,
            _ => return None,
        })
    }
}

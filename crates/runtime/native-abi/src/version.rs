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

pub const NATIVE_ABI_VERSION: NativeAbiVersion = NativeAbiVersion::new(1, 9);
pub const INVALID_HANDLE: u64 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IterationCollectionKind {
    Array = 0,
    Table = 1,
    List = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IterationStatus {
    Failure = 0,
    Item = 1,
    End = 2,
    ConcurrentModification = 3,
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

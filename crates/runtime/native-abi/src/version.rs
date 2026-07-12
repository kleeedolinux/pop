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

pub const NATIVE_ABI_VERSION: NativeAbiVersion = NativeAbiVersion::new(1, 4);
pub const INVALID_HANDLE: u64 = 0;

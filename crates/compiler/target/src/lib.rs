//! Backend-neutral target capabilities and layout requests.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerWidth {
    Bits32,
    Bits64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endianness {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TargetCapability {
    Exceptions,
    TailCalls,
    Threads,
    Atomics,
    Coroutines,
    Simd,
    PreciseStackMaps,
    RelocatingNursery,
    SharedLibraries,
    DebugInformation,
    LlvmBpf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectFormat {
    Elf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatingSystem {
    None,
    Linux,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetSpec {
    triple: String,
    pointer_width: PointerWidth,
    endianness: Endianness,
    object_format: ObjectFormat,
    operating_system: OperatingSystem,
    capabilities: BTreeSet<TargetCapability>,
}

impl TargetSpec {
    #[must_use]
    pub fn builder(triple: impl Into<String>) -> TargetSpecBuilder {
        TargetSpecBuilder {
            triple: triple.into(),
            pointer_width: None,
            endianness: None,
            object_format: None,
            operating_system: None,
            capabilities: BTreeSet::new(),
        }
    }

    /// Returns the built-in target description for a supported triple.
    ///
    /// # Errors
    ///
    /// Returns [`TargetSpecError::UnknownTriple`] when the triple is not part
    /// of Pop Lang's target inventory.
    pub fn for_triple(triple: &str) -> Result<Self, TargetSpecError> {
        match triple {
            "x86_64-unknown-linux-gnu" => Self::builder(triple)
                .pointer_width(PointerWidth::Bits64)
                .endianness(Endianness::Little)
                .object_format(ObjectFormat::Elf)
                .operating_system(OperatingSystem::Linux)
                .capability(TargetCapability::Threads)
                .capability(TargetCapability::PreciseStackMaps)
                .build(),
            "bpfel-unknown-none" => Self::builder(triple)
                .pointer_width(PointerWidth::Bits64)
                .endianness(Endianness::Little)
                .object_format(ObjectFormat::Elf)
                .operating_system(OperatingSystem::None)
                .capability(TargetCapability::LlvmBpf)
                .build(),
            "bpfeb-unknown-none" => Self::builder(triple)
                .pointer_width(PointerWidth::Bits64)
                .endianness(Endianness::Big)
                .object_format(ObjectFormat::Elf)
                .operating_system(OperatingSystem::None)
                .capability(TargetCapability::LlvmBpf)
                .build(),
            _ => Err(TargetSpecError::UnknownTriple),
        }
    }

    #[must_use]
    pub fn triple(&self) -> &str {
        &self.triple
    }

    #[must_use]
    pub const fn pointer_width(&self) -> PointerWidth {
        self.pointer_width
    }

    #[must_use]
    pub const fn endianness(&self) -> Endianness {
        self.endianness
    }

    #[must_use]
    pub const fn object_format(&self) -> ObjectFormat {
        self.object_format
    }

    #[must_use]
    pub const fn operating_system(&self) -> OperatingSystem {
        self.operating_system
    }

    #[must_use]
    pub fn supports(&self, capability: TargetCapability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetSpecBuilder {
    triple: String,
    pointer_width: Option<PointerWidth>,
    endianness: Option<Endianness>,
    object_format: Option<ObjectFormat>,
    operating_system: Option<OperatingSystem>,
    capabilities: BTreeSet<TargetCapability>,
}

impl TargetSpecBuilder {
    #[must_use]
    pub fn pointer_width(mut self, pointer_width: PointerWidth) -> Self {
        self.pointer_width = Some(pointer_width);
        self
    }

    #[must_use]
    pub fn endianness(mut self, endianness: Endianness) -> Self {
        self.endianness = Some(endianness);
        self
    }

    #[must_use]
    pub fn object_format(mut self, object_format: ObjectFormat) -> Self {
        self.object_format = Some(object_format);
        self
    }

    #[must_use]
    pub fn operating_system(mut self, operating_system: OperatingSystem) -> Self {
        self.operating_system = Some(operating_system);
        self
    }

    #[must_use]
    pub fn capability(mut self, capability: TargetCapability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    /// Builds a complete backend-neutral target description.
    ///
    /// # Errors
    ///
    /// Returns [`TargetSpecError`] when the triple is empty or a required
    /// target fact was not supplied.
    pub fn build(self) -> Result<TargetSpec, TargetSpecError> {
        if self.triple.trim().is_empty() {
            return Err(TargetSpecError::EmptyTriple);
        }
        Ok(TargetSpec {
            triple: self.triple,
            pointer_width: self
                .pointer_width
                .ok_or(TargetSpecError::MissingPointerWidth)?,
            endianness: self.endianness.ok_or(TargetSpecError::MissingEndianness)?,
            object_format: self.object_format.unwrap_or(ObjectFormat::Elf),
            operating_system: self.operating_system.unwrap_or(OperatingSystem::None),
            capabilities: self.capabilities,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetSpecError {
    EmptyTriple,
    MissingPointerWidth,
    MissingEndianness,
    UnknownTriple,
}

impl fmt::Display for TargetSpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTriple => formatter.write_str("target triple cannot be empty"),
            Self::MissingPointerWidth => formatter.write_str("target pointer width is required"),
            Self::MissingEndianness => formatter.write_str("target endianness is required"),
            Self::UnknownTriple => formatter.write_str("unknown Pop Lang target triple"),
        }
    }
}

impl Error for TargetSpecError {}

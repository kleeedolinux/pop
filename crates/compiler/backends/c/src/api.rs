//! Stable options, output artifact, and structured errors for C lowering.
use pop_foundation::{BlockId, FunctionId, SymbolId, TypeId, ValueId};
use std::fmt;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CLoweringOptions {
    pub(crate) entry_point: Option<SymbolId>,
}

impl CLoweringOptions {
    #[must_use]
    pub const fn with_entry_point(mut self, symbol: SymbolId) -> Self {
        self.entry_point = Some(symbol);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CTranslationUnit(pub(crate) String);

impl CTranslationUnit {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CTranslationUnit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CBackendError {
    MirVerification(Vec<pop_mir::MirVerificationError>),
    UnsupportedType(TypeId),
    UnsupportedDeclarations,
    UnsupportedEffects(FunctionId),
    UnsupportedAsync(FunctionId),
    UnsupportedInstruction {
        function: FunctionId,
        value: ValueId,
    },
    UnsupportedTerminator {
        function: FunctionId,
        block: BlockId,
    },
    UnsupportedFunctionSignature(SymbolId),
    InvalidEntryPoint(SymbolId),
    UnsupportedEntryPointSignature(SymbolId),
}

impl fmt::Display for CBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MirVerification(errors) => {
                write!(formatter, "MIR verification failed: {errors:?}")
            }
            Self::UnsupportedType(type_id) => {
                write!(
                    formatter,
                    "C backend does not support MIR type t{}",
                    type_id.raw()
                )
            }
            Self::UnsupportedDeclarations => write!(
                formatter,
                "C backend requires the Pop runtime for record, union, class, method, or closure declarations"
            ),
            Self::UnsupportedEffects(function) => write!(
                formatter,
                "C backend does not support the effects of MIR function f{}",
                function.raw()
            ),
            Self::UnsupportedAsync(function) => write!(
                formatter,
                "experimental C backend does not support async function f{}",
                function.raw()
            ),
            Self::UnsupportedInstruction { function, value } => write!(
                formatter,
                "C backend encountered unsupported MIR instruction f{} v{}",
                function.raw(),
                value.raw()
            ),
            Self::UnsupportedTerminator { function, block } => write!(
                formatter,
                "C backend encountered unsupported MIR terminator f{} b{}",
                function.raw(),
                block.raw()
            ),
            Self::UnsupportedFunctionSignature(symbol) => write!(
                formatter,
                "C backend does not support the signature of symbol s{}",
                symbol.raw()
            ),
            Self::InvalidEntryPoint(symbol) => {
                write!(
                    formatter,
                    "C backend cannot find entry symbol s{}",
                    symbol.raw()
                )
            }
            Self::UnsupportedEntryPointSignature(symbol) => write!(
                formatter,
                "C backend requires a no-argument entry returning nothing or Int: s{}",
                symbol.raw()
            ),
        }
    }
}

impl std::error::Error for CBackendError {}

//! Typed callback signature identities carried by canonical MIR.
//!
//! A backend receives the exact ABI, callback function type, target layout
//! bindings, and SHA-256 identity selected before MIR. It never infers a
//! callback ABI from a symbol name or an untyped string.

use core::fmt::Write as _;

use pop_foundation::TypeId;
use pop_runtime_interface::FfiAbiLayoutId;

/// The only callback ABIs admitted by the first stable callback contract.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirFfiCallbackAbi {
    C,
    System,
}

impl MirFfiCallbackAbi {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::System => "System",
        }
    }
}

impl From<pop_types::FfiCallbackAbi> for MirFfiCallbackAbi {
    fn from(value: pop_types::FfiCallbackAbi) -> Self {
        match value {
            pop_types::FfiCallbackAbi::C => Self::C,
            pop_types::FfiCallbackAbi::System => Self::System,
        }
    }
}

/// One exact binary SHA-256 digest, never an unchecked free-form MIR string.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MirFfiCallbackFingerprint([u8; 32]);

impl MirFfiCallbackFingerprint {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    #[must_use]
    pub fn from_lower_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = decode_nibble(pair[0])?
                .checked_mul(16)?
                .checked_add(decode_nibble(pair[1])?)?;
        }
        Some(Self(bytes))
    }

    #[must_use]
    pub fn lower_hex(self) -> String {
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            write!(output, "{byte:02x}").expect("String write cannot fail");
        }
        output
    }
}

const fn decode_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Exact callback signature and target layout bindings verified before a
/// callback operation can reach a backend.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MirFfiCallbackSignature {
    callback_type: TypeId,
    abi: MirFfiCallbackAbi,
    parameter_layouts: Vec<Option<FfiAbiLayoutId>>,
    result_layout: Option<FfiAbiLayoutId>,
    fingerprint: MirFfiCallbackFingerprint,
}

impl MirFfiCallbackSignature {
    #[must_use]
    pub fn new(
        callback_type: TypeId,
        abi: MirFfiCallbackAbi,
        parameter_layouts: Vec<Option<FfiAbiLayoutId>>,
        result_layout: Option<FfiAbiLayoutId>,
        fingerprint: MirFfiCallbackFingerprint,
    ) -> Self {
        Self {
            callback_type,
            abi,
            parameter_layouts,
            result_layout,
            fingerprint,
        }
    }

    #[must_use]
    pub const fn callback_type(&self) -> TypeId {
        self.callback_type
    }

    #[must_use]
    pub const fn abi(&self) -> MirFfiCallbackAbi {
        self.abi
    }

    #[must_use]
    pub fn parameter_layouts(&self) -> &[Option<FfiAbiLayoutId>] {
        &self.parameter_layouts
    }

    #[must_use]
    pub const fn result_layout(&self) -> Option<FfiAbiLayoutId> {
        self.result_layout
    }

    #[must_use]
    pub const fn fingerprint(&self) -> MirFfiCallbackFingerprint {
        self.fingerprint
    }
}

#[cfg(test)]
mod tests {
    use super::MirFfiCallbackFingerprint;

    #[test]
    fn callback_fingerprint_is_exact_binary_lowercase_sha256() {
        let text = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let fingerprint = MirFfiCallbackFingerprint::from_lower_hex(text).expect("valid digest");
        assert_eq!(fingerprint.lower_hex(), text);
        assert!(MirFfiCallbackFingerprint::from_lower_hex(&text.to_uppercase()).is_none());
        assert!(MirFfiCallbackFingerprint::from_lower_hex("00").is_none());
    }
}

use pop_runtime_interface::{RootHandle, RuntimeTypeId};
use pop_runtime_native_abi::CodecEventTag;

pub(super) const CODEC_WRITER_RUNTIME_TYPE: RuntimeTypeId = RuntimeTypeId::new(119);
pub(super) const CODEC_READER_RUNTIME_TYPE: RuntimeTypeId = RuntimeTypeId::new(120);
pub(super) const MAX_CODEC_EVENTS: usize = 65_536;
pub(super) const MAX_CODEC_PAYLOAD_BYTES: usize = 65_535;
pub(super) const MAX_CODEC_LABEL_BYTES: usize = 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StoredScalar {
    Bits(u64),
    String(Vec<u8>),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StoredEvent {
    pub(super) tag: CodecEventTag,
    pub(super) ordinal: u32,
    pub(super) label: Vec<u8>,
    pub(super) auxiliary: u64,
    pub(super) scalar: StoredScalar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CodecCapability {
    Writer {
        /// Fully committed top-level values visible to frozen readers.
        events: Vec<StoredEvent>,
        /// One unpublished top-level aggregate under construction.
        pending: Vec<StoredEvent>,
        containers: Vec<CodecEventTag>,
    },
    Reader {
        events: Vec<StoredEvent>,
        position: usize,
        borrowed_label: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RegisteredCodecCapability {
    pub(super) root: RootHandle,
    pub(super) capability: CodecCapability,
}

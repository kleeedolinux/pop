use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, RuntimeAdapter,
    RuntimeTypeId,
};

use super::model::{
    CODEC_READER_RUNTIME_TYPE, CODEC_WRITER_RUNTIME_TYPE, CodecCapability,
    RegisteredCodecCapability,
};
use crate::state::lock_abi_runtime;

static CODEC_CAPABILITIES: OnceLock<Mutex<BTreeMap<u64, RegisteredCodecCapability>>> =
    OnceLock::new();

pub(super) fn capabilities() -> &'static Mutex<BTreeMap<u64, RegisteredCodecCapability>> {
    CODEC_CAPABILITIES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) fn capability_key(capability: u64, expected: RuntimeTypeId) -> Option<u64> {
    let roots = capabilities()
        .lock()
        .ok()?
        .iter()
        .map(|(key, entry)| (*key, entry.root))
        .collect::<Vec<_>>();
    let mut runtime = lock_abi_runtime().ok()?;
    let reference = ManagedReference::new(capability);
    if runtime.allocation_type(reference) != Some(expected) {
        return None;
    }
    roots
        .into_iter()
        .find_map(|(key, root)| (runtime.resolve_root(root) == Ok(reference)).then_some(key))
}

fn allocate_capability(runtime_type: RuntimeTypeId, capability: CodecCapability) -> u64 {
    let object_map = ObjectMap::scalar(0);
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Ok(reference) = runtime.allocate_object(&ObjectAllocationRequest::new(
        runtime_type,
        AllocationClass::Mature,
        object_map,
    )) else {
        return 0;
    };
    let Ok(root) = runtime.retain_root(reference) else {
        return 0;
    };
    drop(runtime);
    let Ok(mut registered) = capabilities().lock() else {
        if let Ok(mut runtime) = lock_abi_runtime() {
            let _ = runtime.release_root(root);
        }
        return 0;
    };
    registered.insert(
        reference.raw(),
        RegisteredCodecCapability { root, capability },
    );
    reference.raw()
}

/// Creates one sealed native writer capability for a typed format adapter.
#[must_use]
pub fn allocate_codec_writer() -> u64 {
    allocate_capability(
        CODEC_WRITER_RUNTIME_TYPE,
        CodecCapability::Writer {
            events: Vec::new(),
            pending: Vec::new(),
            containers: Vec::new(),
        },
    )
}

/// Freezes a writer tape into a new sealed reader capability.
#[must_use]
pub fn allocate_codec_reader(writer: u64) -> u64 {
    let Some(writer_key) = capability_key(writer, CODEC_WRITER_RUNTIME_TYPE) else {
        return 0;
    };
    let events = {
        let Ok(registered) = capabilities().lock() else {
            return 0;
        };
        let Some(CodecCapability::Writer { events, .. }) =
            registered.get(&writer_key).map(|entry| &entry.capability)
        else {
            return 0;
        };
        events.clone()
    };
    allocate_capability(
        CODEC_READER_RUNTIME_TYPE,
        CodecCapability::Reader {
            events,
            position: 0,
            borrowed_label: Vec::new(),
        },
    )
}

pub(super) fn abort_pending(capability_key: u64) {
    let Ok(mut registered) = capabilities().lock() else {
        return;
    };
    if let Some(CodecCapability::Writer {
        pending,
        containers,
        ..
    }) = registered
        .get_mut(&capability_key)
        .map(|entry| &mut entry.capability)
    {
        pending.clear();
        containers.clear();
    }
}

use pop_runtime_interface::RuntimeOperation;

/// Returns the native C symbol for an operation implemented by ABI 1.5.
///
/// Operations outside the native bootstrap capability set fail closed. MIR and
/// alternate runtime implementations continue to use the semantic operation.
#[must_use]
pub const fn symbol(operation: RuntimeOperation) -> Option<&'static str> {
    match operation {
        RuntimeOperation::AllocateObject => Some("pop_rt_allocate_object"),
        RuntimeOperation::AllocateArray => Some("pop_rt_allocate_array"),
        RuntimeOperation::AllocateArrayFilled => Some("pop_rt_allocate_array_filled"),
        RuntimeOperation::AllocateTable => Some("pop_rt_allocate_table"),
        RuntimeOperation::TupleMake => Some("pop_rt_tuple_make"),
        RuntimeOperation::ArrayGet => Some("pop_rt_array_get"),
        RuntimeOperation::ArrayLength => Some("pop_rt_array_length"),
        RuntimeOperation::ArrayGetChecked => Some("pop_rt_array_get_checked"),
        RuntimeOperation::ArraySet => Some("pop_rt_array_set"),
        RuntimeOperation::ArrayFill => Some("pop_rt_array_fill"),
        RuntimeOperation::FieldGet => Some("pop_rt_field_get"),
        RuntimeOperation::FieldSet => Some("pop_rt_field_set"),
        RuntimeOperation::StringConcat => Some("pop_rt_string_concat"),
        RuntimeOperation::StringFormat => Some("pop_rt_string_format"),
        RuntimeOperation::RetainRoot => Some("pop_rt_retain_root"),
        RuntimeOperation::ReleaseRoot => Some("pop_rt_release_root"),
        RuntimeOperation::Pin => Some("pop_rt_pin"),
        RuntimeOperation::Unpin => Some("pop_rt_unpin"),
        RuntimeOperation::GcSafePoint => Some("pop_rt_gc_safe_point"),
        RuntimeOperation::SatbWriteBarrier => Some("pop_rt_satb_write_barrier"),
        RuntimeOperation::Trap => Some("pop_rt_trap"),
        RuntimeOperation::ContinueUnwind => Some("pop_rt_continue_unwind"),
        RuntimeOperation::RecordUpdate
        | RuntimeOperation::UnionMake
        | RuntimeOperation::CaptureLoad
        | RuntimeOperation::CaptureStore
        | RuntimeOperation::DispatchCall
        | RuntimeOperation::PublishRoots
        | RuntimeOperation::GenerationalWriteBarrier
        | RuntimeOperation::Panic
        | RuntimeOperation::Suspend
        | RuntimeOperation::Resume
        | RuntimeOperation::InitializeModule
        | RuntimeOperation::InitializeBubble => None,
    }
}

use pop_runtime_interface::RuntimeOperation;

/// ABI 1.11 lookup adapter that separates presence from a scalar payload.
pub const TABLE_GET_CHECKED_SYMBOL: &str = "pop_rt_table_get_checked";

/// Returns the native C symbol for an operation implemented by ABI 1.13.
///
/// Operations outside the native bootstrap capability set fail closed. MIR and
/// alternate runtime implementations continue to use the semantic operation.
#[must_use]
pub const fn symbol(operation: RuntimeOperation) -> Option<&'static str> {
    match operation {
        RuntimeOperation::AllocateObject => Some("pop_rt_allocate_object"),
        RuntimeOperation::AllocateObjectInitialized => Some("pop_rt_allocate_initialized_object"),
        RuntimeOperation::AllocateArray => Some("pop_rt_allocate_array"),
        RuntimeOperation::AllocateArrayFilled => Some("pop_rt_allocate_array_filled"),
        RuntimeOperation::AllocateTable => Some("pop_rt_allocate_table"),
        RuntimeOperation::TupleMake => Some("pop_rt_tuple_make"),
        RuntimeOperation::TableGet => Some("pop_rt_table_get"),
        RuntimeOperation::TableSet => Some("pop_rt_table_set"),
        RuntimeOperation::ArrayGet => Some("pop_rt_array_get"),
        RuntimeOperation::ArrayLength => Some("pop_rt_array_length"),
        RuntimeOperation::ArrayGetChecked => Some("pop_rt_array_get_checked"),
        RuntimeOperation::ArraySet => Some("pop_rt_array_set"),
        RuntimeOperation::ArrayFill => Some("pop_rt_array_fill"),
        RuntimeOperation::ListCreate => Some("pop_rt_list_create"),
        RuntimeOperation::ListLength => Some("pop_rt_list_length"),
        RuntimeOperation::ListGet => Some("pop_rt_list_get"),
        RuntimeOperation::ListGetChecked => Some("pop_rt_list_get_checked"),
        RuntimeOperation::ListSet => Some("pop_rt_list_set"),
        RuntimeOperation::ListAdd => Some("pop_rt_list_add"),
        RuntimeOperation::RangeCreate => Some("pop_rt_range_create"),
        RuntimeOperation::IterationAcquire => Some("pop_rt_iteration_acquire"),
        RuntimeOperation::IterationNext => Some("pop_rt_iteration_next"),
        RuntimeOperation::FieldGet => Some("pop_rt_field_get"),
        RuntimeOperation::FieldSet => Some("pop_rt_field_set"),
        RuntimeOperation::StringConcat => Some("pop_rt_string_concat"),
        RuntimeOperation::StringFormat => Some("pop_rt_string_format"),
        RuntimeOperation::RetainRoot => Some("pop_rt_retain_root"),
        RuntimeOperation::ReleaseRoot => Some("pop_rt_release_root"),
        RuntimeOperation::Pin => Some("pop_rt_pin"),
        RuntimeOperation::Unpin => Some("pop_rt_unpin"),
        RuntimeOperation::EnterForeign => Some("pop_rt_enter_foreign"),
        RuntimeOperation::LeaveForeign => Some("pop_rt_leave_foreign"),
        RuntimeOperation::GcSafePoint => Some("pop_rt_gc_safe_point"),
        RuntimeOperation::SatbWriteBarrier => Some("pop_rt_satb_write_barrier"),
        RuntimeOperation::Trap => Some("pop_rt_trap"),
        RuntimeOperation::ContinueUnwind => Some("pop_rt_continue_unwind"),
        RuntimeOperation::CancelSourceCreate => Some("pop_rt_cancel_source_create"),
        RuntimeOperation::CancelSourceToken => Some("pop_rt_cancel_source_token"),
        RuntimeOperation::CancelSourceRelease => Some("pop_rt_cancel_source_release"),
        RuntimeOperation::CancelTokenRelease => Some("pop_rt_cancel_token_release"),
        RuntimeOperation::TaskFrameCreate => Some("pop_rt_task_frame_create"),
        RuntimeOperation::TaskFrameRelease => Some("pop_rt_task_frame_release"),
        RuntimeOperation::TaskFrameLoad => Some("pop_rt_task_frame_load"),
        RuntimeOperation::TaskFrameStore => Some("pop_rt_task_frame_store"),
        RuntimeOperation::TaskFrameSetLiveMap => Some("pop_rt_task_frame_set_live_map"),
        RuntimeOperation::TaskCreate => Some("pop_rt_task_create"),
        RuntimeOperation::TaskStartDirect => Some("pop_rt_task_start_direct"),
        RuntimeOperation::TaskStartGroup => Some("pop_rt_task_start_group"),
        RuntimeOperation::TaskAwait => Some("pop_rt_task_await"),
        RuntimeOperation::TaskCompletionStore => Some("pop_rt_task_completion_store"),
        RuntimeOperation::TaskRelease => Some("pop_rt_task_release"),
        RuntimeOperation::TaskGroupCreate => Some("pop_rt_task_group_create"),
        RuntimeOperation::TaskGroupWrap => Some("pop_rt_task_group_wrap"),
        RuntimeOperation::TaskGroupClose => Some("pop_rt_task_group_close"),
        RuntimeOperation::TaskGroupJoin => Some("pop_rt_task_group_join"),
        RuntimeOperation::Suspend => Some("pop_rt_suspend"),
        RuntimeOperation::Resume => Some("pop_rt_resume"),
        RuntimeOperation::TaskCancel => Some("pop_rt_task_cancel"),
        RuntimeOperation::TaskCancellationRequested => Some("pop_rt_task_cancellation_requested"),
        RuntimeOperation::RecordUpdate
        | RuntimeOperation::UnionMake
        | RuntimeOperation::CaptureLoad
        | RuntimeOperation::CaptureStore
        | RuntimeOperation::DispatchCall
        | RuntimeOperation::PublishRoots
        | RuntimeOperation::GenerationalWriteBarrier
        | RuntimeOperation::Panic
        | RuntimeOperation::InitializeModule
        | RuntimeOperation::InitializeBubble => None,
    }
}

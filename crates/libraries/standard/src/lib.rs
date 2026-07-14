//! Rust implementation foundation for the public `Pop.Standard` Bubble.
//!
//! These APIs are intentionally small, typed, and function-first. They are
//! implementation adapters for the public Pop contracts, not a second source
//! language or a universal object layer.

mod baseline;
mod native_async_net;
mod native_output;
pub mod text;

pub use baseline::{
    ApiBaselineError, ApiKind, ApiStatus, ApiTier, StandardApiBaseline, StandardApiEntry,
    parse_standard_api_baseline, standard_api_baseline,
};
pub use native_async_net::{
    pop_std_net_tcp_accept, pop_std_net_tcp_close, pop_std_net_tcp_listen_loopback,
    pop_std_net_tcp_receive_raw, pop_std_net_tcp_send_all_raw, pop_std_task_cancel_source_cancel,
    pop_std_task_cancel_source_cancellation_requested,
};
pub use native_output::{
    pop_std_print_boolean, pop_std_print_int, pop_std_print_string, pop_std_print_uint64,
    print_string,
};

pub const NATIVE_EXPORTS: &[pop_library_bridge::NativeExport] = &[
    native_output::POP_STD_PRINT_INT_POPLIB_EXPORT,
    native_output::POP_STD_PRINT_STRING_POPLIB_EXPORT,
    native_async_net::POP_STD_TASK_CANCEL_SOURCE_CANCEL_POPLIB_EXPORT,
    native_async_net::POP_STD_TASK_CANCEL_SOURCE_CANCELLATION_REQUESTED_POPLIB_EXPORT,
    native_async_net::POP_STD_NET_TCP_LISTEN_LOOPBACK_POPLIB_EXPORT,
    native_async_net::POP_STD_NET_TCP_ACCEPT_POPLIB_EXPORT,
    native_async_net::POP_STD_NET_TCP_RECEIVE_RAW_POPLIB_EXPORT,
    native_async_net::POP_STD_NET_TCP_SEND_ALL_RAW_POPLIB_EXPORT,
    native_async_net::POP_STD_NET_TCP_CLOSE_POPLIB_EXPORT,
    native_output::POP_STD_PRINT_UINT64_POPLIB_EXPORT,
    native_output::POP_STD_PRINT_BOOLEAN_POPLIB_EXPORT,
];

use pop_library_bridge::{FoundationBubble, NativeEffect, PopAbiType};
use pop_standard::{
    NATIVE_EXPORTS, pop_std_net_tcp_accept, pop_std_net_tcp_close, pop_std_net_tcp_listen_loopback,
    pop_std_net_tcp_receive_raw, pop_std_net_tcp_send_all_raw, pop_std_print_boolean,
    pop_std_print_int, pop_std_print_string, pop_std_print_uint64,
    pop_std_task_cancel_source_cancel, pop_std_task_cancel_source_cancellation_requested,
    print_string,
};

#[test]
fn native_output_adapters_keep_the_bootstrap_signatures() {
    let _: extern "C" fn(i64) = pop_std_print_int;
    let _: extern "C" fn(u64) = pop_std_print_string;
    let _: extern "C" fn(u64) = pop_std_print_uint64;
    let _: extern "C" fn(bool) = pop_std_print_boolean;
    let _: extern "C" fn(u64) -> bool = pop_std_task_cancel_source_cancel;
    let _: extern "C" fn(u64) -> bool = pop_std_task_cancel_source_cancellation_requested;
    let _: extern "C" fn(i64, i64, bool) -> u64 = pop_std_net_tcp_listen_loopback;
    let _: extern "C" fn(u64) -> u64 = pop_std_net_tcp_accept;
    let _: extern "C" fn(u64, u64, u64) -> u64 = pop_std_net_tcp_receive_raw;
    let _: extern "C" fn(u64, u64, u64) -> bool = pop_std_net_tcp_send_all_raw;
    let _: extern "C" fn(u64) -> bool = pop_std_net_tcp_close;
    let _: fn(&str) = print_string;

    assert_eq!(NATIVE_EXPORTS.len(), 26);
    assert!(NATIVE_EXPORTS.iter().all(|export| {
        export.bubble() == FoundationBubble::Standard && export.namespace() == "Pop"
    }));
    assert_eq!(NATIVE_EXPORTS[0].name(), "print");
    assert_eq!(NATIVE_EXPORTS[0].results(), []);
    assert_eq!(NATIVE_EXPORTS[0].effects(), [NativeEffect::AmbientIo]);
    assert_eq!(NATIVE_EXPORTS[0].parameters(), [PopAbiType::Int]);
    assert_eq!(NATIVE_EXPORTS[1].parameters(), [PopAbiType::String]);
    assert_eq!(NATIVE_EXPORTS[5].name(), "Net.Tcp.accept");
    assert_eq!(NATIVE_EXPORTS[5].parameters(), [PopAbiType::UInt64]);
    assert_eq!(NATIVE_EXPORTS[5].results(), [PopAbiType::UInt64]);
    assert_eq!(
        NATIVE_EXPORTS[5].effects(),
        [NativeEffect::AmbientIo, NativeEffect::Suspends]
    );
    assert_eq!(NATIVE_EXPORTS[6].name(), "Net.Tcp.receiveRaw");
    assert_eq!(
        NATIVE_EXPORTS[6].parameters(),
        [PopAbiType::UInt64, PopAbiType::UInt64, PopAbiType::UInt64]
    );
    assert_eq!(NATIVE_EXPORTS[6].results(), [PopAbiType::UInt64]);
    assert_eq!(
        NATIVE_EXPORTS[6].effects(),
        [NativeEffect::AmbientIo, NativeEffect::Suspends]
    );
    assert_eq!(NATIVE_EXPORTS[7].name(), "Net.Tcp.sendAllRaw");
    assert_eq!(
        NATIVE_EXPORTS[7].parameters(),
        [PopAbiType::UInt64, PopAbiType::UInt64, PopAbiType::UInt64]
    );
    assert_eq!(NATIVE_EXPORTS[7].results(), [PopAbiType::Boolean]);
    assert_eq!(
        NATIVE_EXPORTS[7].effects(),
        [NativeEffect::AmbientIo, NativeEffect::Suspends]
    );
    assert_eq!(NATIVE_EXPORTS[9].name(), "print");
    assert_eq!(NATIVE_EXPORTS[9].parameters(), [PopAbiType::UInt64]);
    assert_eq!(NATIVE_EXPORTS[9].results(), []);
    assert_eq!(NATIVE_EXPORTS[9].effects(), [NativeEffect::AmbientIo]);
    assert_eq!(NATIVE_EXPORTS[10].name(), "print");
    assert_eq!(NATIVE_EXPORTS[10].parameters(), [PopAbiType::Boolean]);
    assert_eq!(NATIVE_EXPORTS[10].results(), []);
    assert_eq!(NATIVE_EXPORTS[10].effects(), [NativeEffect::AmbientIo]);
}

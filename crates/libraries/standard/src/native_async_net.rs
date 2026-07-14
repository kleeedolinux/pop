//! Native-backed Task and TCP standard adapters.

use pop_library_bridge::poplib;

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Task.CancelSource.cancel",
    parameters(UInt64),
    results(Boolean),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_task_cancel_source_cancel(token: u64) -> bool {
    pop_runtime_native::pop_rt_task_cancel(token) != 0
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Task.CancelSource.cancellationRequested",
    parameters(UInt64),
    results(Boolean),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_task_cancel_source_cancellation_requested(token: u64) -> bool {
    pop_runtime_native::pop_rt_task_cancellation_requested(token) != 0
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.listenLoopback",
    parameters(Int, Int, Boolean),
    results(UInt64),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_tcp_listen_loopback(port: i64, backlog: i64, reuse: bool) -> u64 {
    let Ok(port) = u16::try_from(port) else {
        return 0;
    };
    let Ok(backlog) = u32::try_from(backlog) else {
        return 0;
    };
    pop_runtime_native::pop_rt_net_tcp_listen_loopback(port, backlog, u8::from(reuse))
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.accept",
    parameters(UInt64),
    results(UInt64),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_accept(listener: u64) -> u64 {
    loop {
        let connection = pop_runtime_native::pop_rt_net_tcp_accept(listener);
        if connection != pop_runtime_native::WOULD_BLOCK {
            return connection;
        }
        std::thread::yield_now();
    }
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.receiveRaw",
    parameters(UInt64, UInt64, UInt64),
    results(UInt64),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_receive_raw(handle: u64, buffer: u64, length: u64) -> u64 {
    let Some(buffer) = std::ptr::NonNull::<u8>::new(buffer as *mut u8) else {
        return 0;
    };
    loop {
        let received =
            unsafe { pop_runtime_native::pop_rt_net_tcp_receive(handle, buffer.as_ptr(), length) };
        if received != pop_runtime_native::WOULD_BLOCK {
            return received;
        }
        std::thread::yield_now();
    }
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.sendAllRaw",
    parameters(UInt64, UInt64, UInt64),
    results(Boolean),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_send_all_raw(handle: u64, buffer: u64, length: u64) -> bool {
    let Some(buffer) = std::ptr::NonNull::<u8>::new(buffer as *mut u8) else {
        return false;
    };
    unsafe { pop_runtime_native::pop_rt_net_tcp_send_all(handle, buffer.as_ptr(), length) != 0 }
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.close",
    parameters(UInt64),
    results(Boolean),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_tcp_close(handle: u64) -> bool {
    pop_runtime_native::pop_rt_net_tcp_close(handle) != 0
}

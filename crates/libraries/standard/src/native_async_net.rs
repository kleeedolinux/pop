//! Native-backed Task and TCP standard adapters.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use pop_library_bridge::poplib;

static NEXT_BUFFER_HANDLE: AtomicU64 = AtomicU64::new(1);
static LAST_NET_ERROR: AtomicU64 = AtomicU64::new(0);
static BUFFERS: OnceLock<Mutex<BTreeMap<u64, Vec<u8>>>> = OnceLock::new();

fn buffers() -> &'static Mutex<BTreeMap<u64, Vec<u8>>> {
    BUFFERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn set_last_error(code: u64) -> u64 {
    LAST_NET_ERROR.store(code, Ordering::Relaxed);
    code
}

fn remember_failure<T>(value: Option<T>, code: u64) -> Option<T> {
    if value.is_none() {
        set_last_error(code);
    }
    value
}

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
    name = "Net.Address.loopback",
    parameters(Int),
    results(NetAddress),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_address_loopback(port: i64) -> u64 {
    let Ok(port) = u16::try_from(port) else {
        set_last_error(1);
        return 0;
    };
    u64::from(port)
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Address.port",
    parameters(NetAddress),
    results(Int),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_address_port(address: u64) -> i64 {
    i64::from(u16::try_from(address).unwrap_or(0))
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.listen",
    parameters(NetAddress, Int, Boolean),
    results(NetTcpListener),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_tcp_listen(address: u64, backlog: i64, reuse: bool) -> u64 {
    let Some(port) = remember_failure(u16::try_from(address).ok(), 1) else {
        return 0;
    };
    let Some(backlog) = remember_failure(u32::try_from(backlog).ok(), 2) else {
        return 0;
    };
    let listener =
        pop_runtime_native::pop_rt_net_tcp_listen_loopback(port, backlog, u8::from(reuse));
    if listener == 0 {
        set_last_error(3);
    }
    listener
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.localPort",
    parameters(NetTcpListener),
    results(Int),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_tcp_local_port(listener: u64) -> i64 {
    let port = pop_runtime_native::pop_rt_net_tcp_listener_port(listener);
    if port == 0 {
        set_last_error(4);
    }
    i64::try_from(port).unwrap_or(0)
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.connectLoopback",
    parameters(Int),
    results(NetTcpConnection),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_connect_loopback(port: i64) -> u64 {
    let Some(port) = remember_failure(u16::try_from(port).ok(), 1) else {
        return 0;
    };
    let connection = pop_runtime_native::pop_rt_net_tcp_connect_loopback(port);
    if connection == 0 {
        set_last_error(5);
    }
    connection
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
    name = "Net.Tcp.accept",
    parameters(NetTcpListener),
    results(NetTcpConnection),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_accept_connection(listener: u64) -> u64 {
    pop_std_net_tcp_accept(listener)
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Buffer.fromString",
    parameters(String),
    results(Buffer),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_buffer_from_string(reference: u64) -> u64 {
    let Some(bytes) = pop_internal::runtime::string_bytes(reference) else {
        set_last_error(6);
        return 0;
    };
    let handle = NEXT_BUFFER_HANDLE.fetch_add(1, Ordering::Relaxed);
    buffers()
        .lock()
        .expect("standard buffer registry poisoned")
        .insert(handle, bytes);
    handle
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "MutableBuffer.create",
    parameters(Int),
    results(MutableBuffer),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_mutable_buffer_create(length: i64) -> u64 {
    let Some(length) = remember_failure(usize::try_from(length).ok(), 7) else {
        return 0;
    };
    let handle = NEXT_BUFFER_HANDLE.fetch_add(1, Ordering::Relaxed);
    buffers()
        .lock()
        .expect("standard buffer registry poisoned")
        .insert(handle, vec![0; length]);
    handle
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Buffer.length",
    parameters(Buffer),
    results(Int),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_buffer_length(buffer: u64) -> i64 {
    buffers()
        .lock()
        .expect("standard buffer registry poisoned")
        .get(&buffer)
        .and_then(|buffer| i64::try_from(buffer.len()).ok())
        .unwrap_or(0)
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
    name = "Net.Tcp.receive",
    parameters(NetTcpConnection, MutableBuffer),
    results(Int),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_receive(connection: u64, buffer: u64) -> i64 {
    loop {
        let received = {
            let mut buffers = buffers().lock().expect("standard buffer registry poisoned");
            let Some(buffer) = buffers.get_mut(&buffer) else {
                set_last_error(8);
                return 0;
            };
            unsafe {
                pop_runtime_native::pop_rt_net_tcp_receive(
                    connection,
                    buffer.as_mut_ptr(),
                    buffer.len() as u64,
                )
            }
        };
        if received != pop_runtime_native::WOULD_BLOCK {
            return i64::try_from(received).unwrap_or(0);
        }
        std::thread::yield_now();
    }
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.sendAll",
    parameters(NetTcpConnection, Buffer),
    results(Boolean),
    effects(AmbientIo, Suspends),
)]
pub extern "C" fn pop_std_net_tcp_send_all(connection: u64, buffer: u64) -> bool {
    let bytes = {
        let buffers = buffers().lock().expect("standard buffer registry poisoned");
        let Some(buffer) = buffers.get(&buffer) else {
            set_last_error(8);
            return false;
        };
        buffer.clone()
    };
    unsafe {
        pop_runtime_native::pop_rt_net_tcp_send_all(connection, bytes.as_ptr(), bytes.len() as u64)
            != 0
    }
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

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.close",
    parameters(NetTcpListener),
    results(Boolean),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_tcp_close_listener(listener: u64) -> bool {
    pop_std_net_tcp_close(listener)
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Tcp.close",
    parameters(NetTcpConnection),
    results(Boolean),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_tcp_close_connection(connection: u64) -> bool {
    pop_std_net_tcp_close(connection)
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Error.lastCode",
    parameters(),
    results(NetError),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_error_last_code() -> u64 {
    LAST_NET_ERROR.load(Ordering::Relaxed)
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "Net.Error.code",
    parameters(NetError),
    results(Int),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_net_error_code(error: u64) -> i64 {
    i64::try_from(error).unwrap_or(0)
}

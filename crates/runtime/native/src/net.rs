//! Native nonblocking TCP runtime boundary.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};

use crate::state::{abi_tcp_listeners, abi_tcp_streams, allocate_net_handle};

pub const WOULD_BLOCK: u64 = u64::MAX;

pub fn tcp_listener_port_for_tests(handle: u64) -> Option<u16> {
    abi_tcp_listeners()
        .lock()
        .expect("tcp listener state poisoned")
        .get(&handle)
        .and_then(|listener| listener.local_addr().ok())
        .map(|address| address.port())
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_net_tcp_listen_loopback(port: u16, _backlog: u32, _reuse: u8) -> u64 {
    let Ok(listener) = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))) else {
        return 0;
    };
    if listener.set_nonblocking(true).is_err() {
        return 0;
    }
    let handle = allocate_net_handle();
    abi_tcp_listeners()
        .lock()
        .expect("tcp listener state poisoned")
        .insert(handle, listener);
    handle
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_net_tcp_accept(listener: u64) -> u64 {
    let listeners = abi_tcp_listeners()
        .lock()
        .expect("tcp listener state poisoned");
    let Some(listener) = listeners.get(&listener) else {
        return 0;
    };
    let Ok((stream, _)) = listener.accept() else {
        return WOULD_BLOCK;
    };
    if stream.set_nonblocking(true).is_err() {
        return 0;
    }
    drop(listeners);
    let handle = allocate_net_handle();
    abi_tcp_streams()
        .lock()
        .expect("tcp stream state poisoned")
        .insert(handle, stream);
    handle
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_net_tcp_receive(
    connection: u64,
    buffer: *mut u8,
    length: u64,
) -> u64 {
    if buffer.is_null() && length != 0 {
        return 0;
    }
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    let mut streams = abi_tcp_streams().lock().expect("tcp stream state poisoned");
    let Some(stream) = streams.get_mut(&connection) else {
        return 0;
    };
    let target = if length == 0 {
        &mut []
    } else {
        // SAFETY: The native backend supplies a mutable buffer valid for
        // `length` bytes. Null is rejected above for nonempty buffers.
        unsafe { std::slice::from_raw_parts_mut(buffer, length) }
    };
    match stream.read(target) {
        Ok(received) => received as u64,
        Err(error) if error.kind() == ErrorKind::WouldBlock => WOULD_BLOCK,
        Err(_) => 0,
    }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_net_tcp_send_all(
    connection: u64,
    buffer: *const u8,
    length: u64,
) -> u8 {
    if buffer.is_null() && length != 0 {
        return 0;
    }
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    let mut streams = abi_tcp_streams().lock().expect("tcp stream state poisoned");
    let Some(stream) = streams.get_mut(&connection) else {
        return 0;
    };
    let source = if length == 0 {
        &[]
    } else {
        // SAFETY: The native backend supplies a readable buffer valid for
        // `length` bytes. Null is rejected above for nonempty buffers.
        unsafe { std::slice::from_raw_parts(buffer, length) }
    };
    match stream.write_all(source) {
        Ok(()) => 1,
        Err(error) if error.kind() == ErrorKind::WouldBlock => 2,
        Err(_) => 0,
    }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_net_tcp_close(handle: u64) -> u8 {
    let removed_stream = abi_tcp_streams()
        .lock()
        .expect("tcp stream state poisoned")
        .remove(&handle)
        .is_some();
    let removed_listener = abi_tcp_listeners()
        .lock()
        .expect("tcp listener state poisoned")
        .remove(&handle)
        .is_some();
    u8::from(removed_stream || removed_listener)
}

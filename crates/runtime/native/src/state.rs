//! Process-global native stable-generational composition state.

use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};

use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::ArrayElementMap;

static ABI_RUNTIME: OnceLock<Mutex<StableGenerationalRuntime>> = OnceLock::new();
static ABI_TABLES: OnceLock<Mutex<BTreeMap<u64, TableMetadata>>> = OnceLock::new();
static ABI_LISTS: OnceLock<Mutex<BTreeMap<u64, ListMetadata>>> = OnceLock::new();
static ABI_TCP_LISTENERS: OnceLock<Mutex<BTreeMap<u64, TcpListener>>> = OnceLock::new();
static ABI_TCP_STREAMS: OnceLock<Mutex<BTreeMap<u64, TcpStream>>> = OnceLock::new();
static ABI_NET_NEXT_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableMetadata {
    pub(crate) length: u32,
    pub(crate) capacity: u32,
    pub(crate) key_map: ArrayElementMap,
    pub(crate) value_map: ArrayElementMap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ListMetadata {
    pub(crate) length: u32,
    pub(crate) capacity: u32,
    pub(crate) element_map: ArrayElementMap,
}

pub(crate) fn abi_runtime() -> &'static Mutex<StableGenerationalRuntime> {
    ABI_RUNTIME.get_or_init(|| Mutex::new(StableGenerationalRuntime::new()))
}

pub(crate) fn abi_tables() -> &'static Mutex<BTreeMap<u64, TableMetadata>> {
    ABI_TABLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn abi_lists() -> &'static Mutex<BTreeMap<u64, ListMetadata>> {
    ABI_LISTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn abi_tcp_listeners() -> &'static Mutex<BTreeMap<u64, TcpListener>> {
    ABI_TCP_LISTENERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn abi_tcp_streams() -> &'static Mutex<BTreeMap<u64, TcpStream>> {
    ABI_TCP_STREAMS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn allocate_net_handle() -> u64 {
    let mut next = ABI_NET_NEXT_HANDLE
        .get_or_init(|| Mutex::new(1))
        .lock()
        .expect("net handle state poisoned");
    let handle = *next;
    *next = next.saturating_add(1).max(1);
    handle
}

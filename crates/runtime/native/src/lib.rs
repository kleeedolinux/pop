//! Native PLRI implementation boundary.
//!
//! This crate currently provides the architecture's Stage-1 bootstrap
//! collector: a precise, stop-the-world mark/sweep heap backed by opaque
//! handles. It deliberately does not claim a moving nursery, concurrent
//! marking, or production write barriers.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_char};
use std::sync::{Mutex, OnceLock};

use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, CollectionStatistics,
    GarbageCollectorContract, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    PanicPayload, RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, RuntimeTypeId,
    SafePointOutcome, WriteBarrier,
};

static ABI_RUNTIME: OnceLock<Mutex<BootstrapRuntime>> = OnceLock::new();

fn abi_runtime() -> &'static Mutex<BootstrapRuntime> {
    ABI_RUNTIME.get_or_init(|| Mutex::new(BootstrapRuntime::new()))
}

/// C-compatible bootstrap runtime identity. The bootstrap collector is
/// intentionally versioned separately from the future production collector.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_major() -> u16 {
    1
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_minor() -> u16 {
    1
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_gc_stage() -> u8 {
    1
}

/// Allocates a scalar array and returns its opaque managed handle, or zero on
/// a typed runtime failure.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_array(length: u64, managed: u8) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(0),
        AllocationClass::Mature,
        length,
        if managed == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
    );
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_array(&request)
        .map_or(0, ManagedReference::raw)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_object(slot_count: u64) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    abi_allocate_object(slot_count)
}

/// Allocates an object using explicit zero-based managed-reference slots.
#[must_use]
pub fn allocate_mapped_object(slot_count: u64, reference_slots: &[u32]) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    let slots = reference_slots
        .iter()
        .copied()
        .map(ObjectSlot::new)
        .collect();
    let Ok(object_map) = ObjectMap::new(slot_count, slots) else {
        return 0;
    };
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_object(&request)
        .map_or(0, ManagedReference::raw)
}

/// C-compatible mapped-object allocation boundary used by native LLVM code.
///
/// # Safety
///
/// When `reference_count` is nonzero, `reference_slots` must address that many
/// readable `u32` slot indices for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_allocate_mapped_object(
    slot_count: u64,
    reference_slots: *const u32,
    reference_count: u64,
) -> u64 {
    let Ok(reference_count) = usize::try_from(reference_count) else {
        return 0;
    };
    if reference_count == 0 {
        return allocate_mapped_object(slot_count, &[]);
    }
    if reference_slots.is_null() {
        return 0;
    }
    // SAFETY: The backend passes a stack array containing exactly the declared
    // number of immutable slot indices.
    let reference_slots = unsafe { std::slice::from_raw_parts(reference_slots, reference_count) };
    allocate_mapped_object(slot_count, reference_slots)
}

fn abi_allocate_object(slot_count: u32) -> u64 {
    let Ok(object_map) = ObjectMap::new(slot_count, Vec::new()) else {
        return 0;
    };
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_object(&request)
        .map_or(0, ManagedReference::raw)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_table(length: u64) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    abi_allocate_object(length)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_tuple_make(length: u64) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    abi_allocate_object(length)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_get(reference: u64, index: u64) -> u64 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .load_array_value(ManagedReference::new(reference), slot)
        .unwrap_or(0)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_set(reference: u64, index: u64, value: u64) -> u8 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(
        runtime
            .store_array_value(ManagedReference::new(reference), slot, value)
            .is_ok(),
    )
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_field_get(reference: u64, field: u64) -> u64 {
    let Some(slot) = array_slot(field) else {
        return 0;
    };
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .load_array_value(ManagedReference::new(reference), slot)
        .unwrap_or(0)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_field_set(reference: u64, field: u64, value: u64) -> u8 {
    let Some(slot) = array_slot(field) else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(
        runtime
            .store_array_value(ManagedReference::new(reference), slot, value)
            .is_ok(),
    )
}

/// Materializes one immutable, valid UTF-8 string from compiler-emitted bytes.
///
/// # Safety
///
/// `bytes` must address `length` readable bytes for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_string_literal(bytes: *const u8, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    if bytes.is_null() {
        return 0;
    }
    // SAFETY: The native backend supplies a pointer to an immutable LLVM
    // constant with exactly the declared byte length.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    allocate_utf8_string_literal(bytes)
}

/// Safe Rust adapter for the bootstrap string-literal ABI.
#[must_use]
pub fn allocate_utf8_string_literal(bytes: &[u8]) -> u64 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    allocate_utf8_string(&mut runtime, bytes).map_or(0, ManagedReference::raw)
}

/// Reads one immutable UTF-8 string through the bootstrap handle boundary.
///
/// The return value is the byte length plus one, reserving zero as failure.
/// Passing a null target queries the required length without copying. A
/// non-null target must provide at least the complete byte length; failures do
/// not write partial data.
///
/// # Safety
///
/// When `target` is non-null, it must address `capacity` writable bytes for the
/// duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_string_read(reference: u64, target: *mut u8, capacity: u64) -> u64 {
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Some(allocation) = runtime.objects.get(&ManagedReference::new(reference)) else {
        return 0;
    };
    if allocation.type_id != RuntimeTypeId::new(1) {
        return 0;
    }
    let mut bytes = Vec::with_capacity(allocation.slots.len());
    for slot in &allocation.slots {
        let SlotValue::Scalar(value) = slot else {
            return 0;
        };
        let Ok(byte) = u8::try_from(*value) else {
            return 0;
        };
        bytes.push(byte);
    }
    if std::str::from_utf8(&bytes).is_err() {
        return 0;
    }
    let Ok(length) = u64::try_from(bytes.len()) else {
        return 0;
    };
    let Some(encoded_length) = length.checked_add(1) else {
        return 0;
    };
    if target.is_null() {
        return encoded_length;
    }
    if capacity < length {
        return 0;
    }
    // SAFETY: The caller contract guarantees a writable buffer of at least
    // `length` bytes, and `bytes` owns exactly that many initialized bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
    encoded_length
}

fn allocate_utf8_string(
    runtime: &mut BootstrapRuntime,
    bytes: &[u8],
) -> Result<ManagedReference, RuntimeFailure> {
    std::str::from_utf8(bytes).map_err(|_| RuntimeFailure::runtime_invariant())?;
    let portable_length =
        u32::try_from(bytes.len()).map_err(|_| RuntimeFailure::runtime_invariant())?;
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::Mature,
        portable_length,
        ArrayElementMap::Scalar,
    );
    let reference = runtime.allocate_array(&request)?;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let index = u32::try_from(index).map_err(|_| RuntimeFailure::runtime_invariant())?;
        runtime.store_scalar(reference, ObjectSlot::new(index), u64::from(byte))?;
    }
    Ok(reference)
}

/// Materializes the valid UTF-8 arguments that follow the executable path.
///
/// The returned array uses a precise managed-reference element map. Zero is
/// returned when any argument is invalid UTF-8 or allocation fails.
#[must_use]
pub fn allocate_process_arguments(arguments: &[&[u8]]) -> u64 {
    if arguments
        .iter()
        .any(|argument| std::str::from_utf8(argument).is_err())
    {
        return 0;
    }
    let Ok(length) = u32::try_from(arguments.len()) else {
        return 0;
    };
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(2),
        AllocationClass::Mature,
        length,
        ArrayElementMap::ManagedReference,
    );
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Ok(array) = runtime.allocate_array(&request) else {
        return 0;
    };
    let Ok(root) = runtime.retain_root(array) else {
        return 0;
    };
    let result = arguments.iter().enumerate().try_for_each(|(index, bytes)| {
        let index = u32::try_from(index).map_err(|_| RuntimeFailure::runtime_invariant())?;
        let string = allocate_utf8_string(&mut runtime, bytes)?;
        runtime.store_array_value(array, ObjectSlot::new(index), string.raw())
    });
    let released = runtime.release_root(root);
    if result.is_err() || released.is_err() {
        0
    } else {
        array.raw()
    }
}

/// Adapts a complete platform argument vector and omits its executable path.
#[must_use]
pub fn allocate_platform_arguments(arguments: &[&CStr]) -> u64 {
    let bytes: Vec<_> = arguments
        .iter()
        .skip(1)
        .map(|argument| argument.to_bytes())
        .collect();
    allocate_process_arguments(&bytes)
}

/// Converts the platform `main` argument vector into Pop Lang's canonical
/// managed `Array<String>`, excluding the executable path.
///
/// # Safety
///
/// `arguments` must point to `argument_count` readable C-string pointers as
/// supplied to the platform process entry. Each non-null pointer must remain
/// valid for the duration of the call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_process_arguments(
    argument_count: i32,
    arguments: *const *const c_char,
) -> u64 {
    let Ok(argument_count) = usize::try_from(argument_count) else {
        return 0;
    };
    if argument_count == 0 {
        return allocate_process_arguments(&[]);
    }
    if arguments.is_null() {
        return 0;
    }
    // SAFETY: The platform provides exactly `argument_count` C-string pointers
    // to `main`; the executable path occupies slot zero.
    let arguments = unsafe { std::slice::from_raw_parts(arguments, argument_count) };
    let mut platform_arguments = Vec::with_capacity(argument_count);
    for argument in arguments {
        if argument.is_null() {
            return 0;
        }
        // SAFETY: Each platform argument is a non-null, nul-terminated string.
        platform_arguments.push(unsafe { CStr::from_ptr(*argument) });
    }
    allocate_platform_arguments(&platform_arguments)
}

/// Compares two managed UTF-8 strings by their byte content.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_string_equal(left: u64, right: u64) -> u8 {
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(runtime.strings_equal(ManagedReference::new(left), ManagedReference::new(right)))
}

fn array_slot(index: u64) -> Option<ObjectSlot> {
    (index > 0)
        .then(|| u32::try_from(index - 1).ok())
        .flatten()
        .map(ObjectSlot::new)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_retain_root(reference: u64) -> u64 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .retain_root(ManagedReference::new(reference))
        .map_or(0, RootHandle::raw)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_release_root(root: u64) -> u8 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(runtime.release_root(RootHandle::new(root)).is_ok())
}

#[must_use]
pub fn request_abi_collection() -> bool {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return false;
    };
    runtime.request_collection();
    true
}

pub fn abi_safe_point(safe_point: u32, roots: &[u64]) -> u8 {
    let root_slots = (0..roots.len())
        .filter_map(|index| u32::try_from(index).ok())
        .map(pop_runtime_interface::RootSlot::new)
        .collect();
    let Ok(stack_map) = pop_runtime_interface::StackMap::new(
        pop_runtime_interface::SafePointId::new(safe_point),
        root_slots,
    ) else {
        return 0;
    };
    let roots = roots
        .iter()
        .copied()
        .map(|root| (root != 0).then(|| ManagedReference::new(root)))
        .collect();
    let Ok(publication) = RootPublication::new(stack_map, roots) else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(runtime.safe_point(&publication).is_ok())
}

/// Publishes exact live managed handles for one native safe point.
///
/// # Safety
///
/// When `root_count` is nonzero, `roots` must address that many readable `u64`
/// managed handles for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_gc_safe_point(
    safe_point: u32,
    roots: *const u64,
    root_count: u64,
) -> u8 {
    let Ok(root_count) = usize::try_from(root_count) else {
        return 0;
    };
    if root_count == 0 {
        return abi_safe_point(safe_point, &[]);
    }
    if roots.is_null() {
        return 0;
    }
    // SAFETY: The backend passes a stack array containing the declared number
    // of live managed handles.
    let roots = unsafe { std::slice::from_raw_parts(roots, root_count) };
    abi_safe_point(safe_point, roots)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_satb_write_barrier(owner: u64) {
    if let Ok(runtime) = abi_runtime().lock() {
        let _ = runtime.contains(ManagedReference::new(owner));
    }
}

/// Terminates native execution for a verified MIR trap edge.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_trap() -> ! {
    std::process::abort()
}

/// Terminates the bootstrap process when a panic unwind reaches the native
/// runtime boundary. Typed expected failures do not use this path.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_continue_unwind() -> ! {
    std::process::abort()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeapLimits {
    maximum_objects: usize,
    maximum_slots: usize,
}

impl HeapLimits {
    #[must_use]
    pub const fn new(maximum_objects: usize, maximum_slots: usize) -> Self {
        Self {
            maximum_objects,
            maximum_slots,
        }
    }

    #[must_use]
    pub const fn maximum_objects(self) -> usize {
        self.maximum_objects
    }

    #[must_use]
    pub const fn maximum_slots(self) -> usize {
        self.maximum_slots
    }
}

impl Default for HeapLimits {
    fn default() -> Self {
        Self::new(usize::MAX, usize::MAX)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotValue {
    Scalar(u64),
    Reference(Option<ManagedReference>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Allocation {
    type_id: RuntimeTypeId,
    class: AllocationClass,
    object_map: ObjectMap,
    slots: Vec<SlotValue>,
}

pub struct BootstrapRuntime {
    objects: BTreeMap<ManagedReference, Allocation>,
    roots: BTreeMap<RootHandle, ManagedReference>,
    next_reference: u64,
    next_root: u64,
    slot_count: usize,
    limits: HeapLimits,
    collection_requested: bool,
}

impl BootstrapRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(HeapLimits::default())
    }

    #[must_use]
    pub fn with_limits(limits: HeapLimits) -> Self {
        Self {
            objects: BTreeMap::new(),
            roots: BTreeMap::new(),
            next_reference: 1,
            next_root: 1,
            slot_count: 0,
            limits,
            collection_requested: false,
        }
    }

    #[must_use]
    pub const fn limits(&self) -> HeapLimits {
        self.limits
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    #[must_use]
    pub const fn slot_count(&self) -> usize {
        self.slot_count
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.objects.contains_key(&reference)
    }

    #[must_use]
    pub fn allocation_type(&self, reference: ManagedReference) -> Option<RuntimeTypeId> {
        self.objects
            .get(&reference)
            .map(|allocation| allocation.type_id)
    }

    #[must_use]
    pub fn allocation_class(&self, reference: ManagedReference) -> Option<AllocationClass> {
        self.objects
            .get(&reference)
            .map(|allocation| allocation.class)
    }

    pub const fn request_collection(&mut self) {
        self.collection_requested = true;
    }

    /// Stores a managed reference into a slot identified as a reference by the
    /// allocation's precise object map.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for invalid objects, slots, or
    /// references.
    pub fn store_reference(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        if value.is_some_and(|reference| !self.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let previous = self
            .objects
            .get(&owner)
            .and_then(|allocation| allocation.slots.get(slot.raw() as usize))
            .copied();
        let Some(SlotValue::Reference(previous)) = previous else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        self.write_barrier(WriteBarrier::new(
            pop_runtime_interface::BarrierKind::CombinedSatbGenerational,
            owner,
            slot,
            previous,
            value,
        ))?;
        let allocation = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        allocation.slots[slot.raw() as usize] = SlotValue::Reference(value);
        Ok(())
    }

    /// Stores a scalar into a slot that is absent from the precise pointer map.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for invalid objects, slots, or a
    /// reference-designated slot.
    pub fn store_scalar(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let allocation = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let Some(current) = allocation.slots.get_mut(slot.raw() as usize) else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if !matches!(current, SlotValue::Scalar(_)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        *current = SlotValue::Scalar(value);
        Ok(())
    }

    /// Loads a scalar from a precise non-reference slot.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for invalid objects, slots, or
    /// reference-designated slots.
    pub fn load_scalar(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        let allocation = self
            .objects
            .get(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match allocation.slots.get(slot.raw() as usize) {
            Some(SlotValue::Scalar(value)) => Ok(*value),
            Some(SlotValue::Reference(_)) | None => Err(RuntimeFailure::runtime_invariant()),
        }
    }

    /// Stores either a scalar or a managed handle according to the slot's
    /// precise allocation map.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for an invalid allocation, slot, or
    /// managed handle.
    pub fn store_array_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let is_reference = self
            .objects
            .get(&owner)
            .and_then(|allocation| allocation.slots.get(slot.raw() as usize))
            .is_some_and(|slot| matches!(slot, SlotValue::Reference(_)));
        if is_reference {
            self.store_reference(
                owner,
                slot,
                (value != 0).then(|| ManagedReference::new(value)),
            )
        } else {
            self.store_scalar(owner, slot, value)
        }
    }

    /// Loads either a scalar or a managed handle according to the slot's
    /// precise allocation map. Empty references are returned as zero.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for an invalid allocation or slot.
    pub fn load_array_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        let allocation = self
            .objects
            .get(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match allocation.slots.get(slot.raw() as usize) {
            Some(SlotValue::Scalar(value)) => Ok(*value),
            Some(SlotValue::Reference(value)) => Ok(value.map_or(0, ManagedReference::raw)),
            None => Err(RuntimeFailure::runtime_invariant()),
        }
    }

    fn strings_equal(&self, left: ManagedReference, right: ManagedReference) -> bool {
        let Some(left) = self.objects.get(&left) else {
            return false;
        };
        let Some(right) = self.objects.get(&right) else {
            return false;
        };
        left.type_id == RuntimeTypeId::new(1)
            && right.type_id == RuntimeTypeId::new(1)
            && left.slots == right.slots
    }

    /// Performs a precise stop-the-world collection using registered strong
    /// roots plus the stack roots published for this safe point.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic if a root or traced edge names an
    /// invalid managed reference.
    pub fn collect(
        &mut self,
        stack_roots: &RootPublication,
    ) -> Result<CollectionStatistics, RuntimeFailure> {
        let mut roots: Vec<_> = self.roots.values().copied().collect();
        roots.extend(stack_roots.managed_references());
        self.collect_references(&roots)
    }

    fn allocate(
        &mut self,
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        object_map: ObjectMap,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let requested_slots = usize::try_from(object_map.slot_count())
            .map_err(|_| Self::out_of_memory(1, usize::MAX))?;
        self.ensure_capacity(requested_slots)?;
        let reference = ManagedReference::new(self.next_reference);
        self.next_reference = self
            .next_reference
            .checked_add(1)
            .ok_or_else(|| Self::out_of_memory(1, requested_slots))?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(requested_slots)
            .map_err(|_| Self::out_of_memory(1, requested_slots))?;
        for index in 0..object_map.slot_count() {
            slots.push(if object_map.is_reference_slot(ObjectSlot::new(index)) {
                SlotValue::Reference(None)
            } else {
                SlotValue::Scalar(0)
            });
        }
        self.objects.insert(
            reference,
            Allocation {
                type_id,
                class: allocation_class,
                object_map,
                slots,
            },
        );
        self.slot_count += requested_slots;
        Ok(reference)
    }

    fn ensure_capacity(&mut self, requested_slots: usize) -> Result<(), RuntimeFailure> {
        if self.has_capacity(requested_slots) {
            return Ok(());
        }
        let registered_roots: Vec<_> = self.roots.values().copied().collect();
        self.collect_references(&registered_roots)?;
        if self.has_capacity(requested_slots) {
            Ok(())
        } else {
            Err(Self::out_of_memory(1, requested_slots))
        }
    }

    fn has_capacity(&self, requested_slots: usize) -> bool {
        self.objects.len() < self.limits.maximum_objects
            && self
                .slot_count
                .checked_add(requested_slots)
                .is_some_and(|slots| slots <= self.limits.maximum_slots)
    }

    fn collect_references(
        &mut self,
        roots: &[ManagedReference],
    ) -> Result<CollectionStatistics, RuntimeFailure> {
        let before = self.objects.len();
        let mut marked = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(reference) = pending.pop() {
            if !marked.insert(reference) {
                continue;
            }
            let allocation = self
                .objects
                .get(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            for slot in allocation.object_map.reference_slots() {
                match allocation.slots.get(slot.raw() as usize) {
                    Some(SlotValue::Reference(Some(child))) => pending.push(*child),
                    Some(SlotValue::Reference(None)) => {}
                    Some(SlotValue::Scalar(_)) | None => {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                }
            }
        }

        self.objects
            .retain(|reference, _| marked.contains(reference));
        self.slot_count = self
            .objects
            .values()
            .map(|allocation| allocation.slots.len())
            .sum();
        let live = self.objects.len();
        Ok(CollectionStatistics::new(
            portable_count(live),
            portable_count(before - live),
            portable_count(marked.len()),
        ))
    }

    fn validate_reference(&self, reference: ManagedReference) -> Result<(), RuntimeFailure> {
        if self.contains(reference) {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }

    fn out_of_memory(requested_objects: usize, requested_slots: usize) -> RuntimeFailure {
        RuntimeFailure::from_panic(PanicPayload::out_of_memory(
            portable_count(requested_objects),
            portable_count(requested_slots),
        ))
    }
}

fn portable_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

impl Default for BootstrapRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeAdapter for BootstrapRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::bootstrap_stage1()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate(
            request.type_id(),
            request.allocation_class(),
            request.object_map().clone(),
        )
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference_slots = match request.element_map() {
            ArrayElementMap::Scalar => Vec::new(),
            ArrayElementMap::ManagedReference => {
                let length = usize::try_from(request.length())
                    .map_err(|_| Self::out_of_memory(1, usize::MAX))?;
                let mut slots = Vec::new();
                slots
                    .try_reserve_exact(length)
                    .map_err(|_| Self::out_of_memory(1, length))?;
                slots.extend((0..request.length()).map(ObjectSlot::new));
                slots
            }
        };
        let object_map = ObjectMap::new(request.length(), reference_slots)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        self.allocate(request.type_id(), request.allocation_class(), object_map)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.validate_reference(reference)?;
        let root = RootHandle::new(self.next_root);
        self.next_root = self
            .next_root
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.roots.insert(root, reference);
        Ok(root)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.roots
            .remove(&root)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn safe_point(&mut self, roots: &RootPublication) -> Result<SafePointOutcome, RuntimeFailure> {
        for reference in roots.managed_references() {
            self.validate_reference(reference)?;
        }
        if !self.collection_requested {
            return Ok(SafePointOutcome::no_collection());
        }
        self.collection_requested = false;
        self.collect(roots).map(SafePointOutcome::collected)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.validate_reference(barrier.owner())?;
        if let Some(reference) = barrier.previous() {
            self.validate_reference(reference)?;
        }
        if let Some(reference) = barrier.value() {
            self.validate_reference(reference)?;
        }
        let allocation = self
            .objects
            .get(&barrier.owner())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let current = allocation.slots.get(barrier.slot().raw() as usize);
        if current != Some(&SlotValue::Reference(barrier.previous())) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        Ok(())
    }
}

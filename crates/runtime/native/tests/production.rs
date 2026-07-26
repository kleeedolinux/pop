#![cfg(feature = "production-generational")]

use pop_runtime_interface::AllocationClass;
use pop_runtime_native::{
    native_allocation_class, pop_rt_abi_major, pop_rt_abi_minor,
    pop_rt_allocate_initialized_object_at_site, pop_rt_allocate_object,
    pop_rt_attach_managed_thread, pop_rt_detach_managed_thread, pop_rt_field_get, pop_rt_field_set,
    pop_rt_gc_safe_point_v2, pop_rt_gc_stage, pop_rt_supports_abi, request_abi_relocation,
};
use pop_runtime_native_abi::AllocationSiteDescriptorAbi;

#[test]
#[allow(unsafe_code)]
fn production_facade_selects_abi_two_and_rewrites_a_forced_native_root() {
    assert_eq!((pop_rt_abi_major(), pop_rt_abi_minor()), (2, 0));
    assert_eq!(pop_rt_supports_abi(2, 0), 1);
    assert_eq!(pop_rt_supports_abi(1, 22), 0);
    assert_eq!(pop_rt_gc_stage(), 3);

    let binding = pop_rt_attach_managed_thread(1);
    assert_ne!(binding, 0);
    let old = pop_rt_allocate_object(1);
    assert_ne!(old, 0);
    assert_eq!(
        native_allocation_class(old),
        Some(AllocationClass::NurseryEligible)
    );
    assert_eq!(pop_rt_field_set(old, 1, 42), 1);
    let mut roots = [old];
    assert!(request_abi_relocation());
    // SAFETY: `roots` is one live writable token slot for the complete call.
    assert_eq!(
        unsafe { pop_rt_gc_safe_point_v2(91, roots.as_mut_ptr(), roots.len() as u64) },
        1
    );
    assert_ne!(roots[0], old, "the production nursery must relocate");
    assert_eq!(
        pop_rt_field_get(old, 1),
        0,
        "the stale token must fail closed"
    );
    assert_eq!(pop_rt_field_get(roots[0], 1), 42);

    let descriptor = AllocationSiteDescriptorAbi {
        bubble: 1,
        owner: 2,
        site: 3,
        runtime_type: 4,
        allocation_class: 0,
        reserved: [0; 3],
        slot_count: 1,
        reference_count: 0,
        reference_slots: std::ptr::null(),
    };
    let value = [7_u64];
    for _ in 0..2 {
        // SAFETY: the descriptor and value slice remain live for the complete call.
        let reference = unsafe {
            pop_rt_allocate_initialized_object_at_site(
                &raw const descriptor,
                value.as_ptr(),
                value.len() as u64,
            )
        };
        assert_ne!(reference, 0);
        assert_eq!(
            native_allocation_class(reference),
            Some(AllocationClass::NurseryEligible),
            "production allocation sites must not refill stable-token mature TLABs"
        );
    }
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);
}

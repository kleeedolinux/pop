use std::sync::{Mutex, MutexGuard, OnceLock};

use pop_runtime_native::{
    allocation_site_tlab_refill_count, pop_rt_allocate_initialized_object_at_site,
    pop_rt_allocate_object, pop_rt_attach_managed_thread, pop_rt_detach_managed_thread,
    pop_rt_enter_foreign, pop_rt_field_get, pop_rt_leave_foreign,
};
use pop_runtime_native_abi::AllocationSiteDescriptorAbi;

fn binding_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("managed binding ABI test lock")
}

#[test]
#[allow(unsafe_code)]
fn managed_thread_attachment_is_balanced_and_guards_foreign_cleanup() {
    let _guard = binding_test_lock();
    assert_eq!(pop_rt_attach_managed_thread(0), 0);
    let binding = pop_rt_attach_managed_thread(1);
    assert_ne!(binding, 0);
    assert_eq!(pop_rt_attach_managed_thread(1), 0);

    let root = pop_rt_allocate_object(0);
    assert_ne!(root, 0);
    let mut roots = [root];
    let transition = unsafe { pop_rt_enter_foreign(60, roots.as_mut_ptr(), 1, 0) };
    assert_ne!(transition, 0);
    assert_eq!(pop_rt_detach_managed_thread(binding), 0);
    assert_eq!(
        unsafe { pop_rt_leave_foreign(transition, roots.as_mut_ptr(), 1) },
        1
    );
    assert_eq!(
        std::thread::spawn(move || pop_rt_detach_managed_thread(binding))
            .join()
            .expect("wrong-thread detach probe"),
        0
    );
    assert_eq!(pop_rt_detach_managed_thread(binding + 1), 0);
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);
    assert_eq!(pop_rt_detach_managed_thread(binding), 0);
}

#[test]
#[allow(unsafe_code)]
fn bound_fixed_layout_allocations_reuse_one_tlab_lease_and_flush_on_detach() {
    let _guard = binding_test_lock();
    let binding = pop_rt_attach_managed_thread(1);
    assert_ne!(binding, 0);
    let descriptor = AllocationSiteDescriptorAbi {
        bubble: 0x00ff_0101,
        owner: 1,
        site: 1,
        runtime_type: 101,
        allocation_class: 1,
        reserved: [0; 3],
        slot_count: 2,
        reference_count: 0,
        reference_slots: std::ptr::null(),
    };
    let first_value = [0x00ff_0101_u64, 1_u64];
    let refills = allocation_site_tlab_refill_count();
    let first = unsafe {
        pop_rt_allocate_initialized_object_at_site(&raw const descriptor, first_value.as_ptr(), 2)
    };
    assert_ne!(first, 0);
    assert_eq!(
        allocation_site_tlab_refill_count(),
        refills,
        "a one-shot allocation site must not reserve a full TLAB"
    );
    assert_eq!(pop_rt_field_get(first, 2), 1);
    let mut references = vec![first];
    for value in 2..=8_192_u64 {
        let values = [0x00ff_0101, value];
        references.push(unsafe {
            pop_rt_allocate_initialized_object_at_site(&raw const descriptor, values.as_ptr(), 2)
        });
    }
    assert!(references.iter().all(|reference| *reference != 0));
    assert_eq!(allocation_site_tlab_refill_count(), refills + 4);
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);

    for (index, reference) in references.into_iter().enumerate() {
        assert_eq!(pop_rt_field_get(reference, 2), index as u64 + 1);
    }
}

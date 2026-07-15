use std::sync::{Mutex, MutexGuard, OnceLock};

use pop_runtime_native::{
    pop_rt_allocate_object, pop_rt_attach_managed_thread, pop_rt_detach_managed_thread,
    pop_rt_enter_foreign, pop_rt_leave_foreign,
};

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

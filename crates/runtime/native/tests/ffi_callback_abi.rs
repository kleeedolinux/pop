use pop_runtime_native::{
    pop_rt_allocate_object, pop_rt_attach_managed_thread, pop_rt_detach_managed_thread,
    pop_rt_enter_foreign, pop_rt_ffi_callback_close, pop_rt_ffi_callback_enter,
    pop_rt_ffi_callback_leave, pop_rt_ffi_callback_open, pop_rt_leave_foreign,
};

#[test]
#[allow(unsafe_code)]
fn calling_thread_callback_balances_state_and_invalidates_context() {
    let binding = pop_rt_attach_managed_thread(1);
    assert_ne!(binding, 0);
    let environment = pop_rt_allocate_object(0);
    assert_ne!(environment, 0);

    let mut context = 0xaaaa;
    let registration =
        unsafe { pop_rt_ffi_callback_open(environment, 41, 1, 0, 0, &raw mut context) };
    assert_ne!(registration, 0);
    assert_ne!(context, 0);
    assert_ne!(context, 0xaaaa);

    let mut callback_environment = 0xbbbb;
    assert_eq!(
        unsafe { pop_rt_ffi_callback_enter(context, 41, &raw mut callback_environment) },
        0
    );
    assert_eq!(callback_environment, 0xbbbb);

    let mut roots = [environment];
    let bounded = unsafe { pop_rt_enter_foreign(42, roots.as_mut_ptr(), 1, 1) };
    assert_ne!(bounded, 0);
    let mut bounded_environment = 0xb0b0;
    assert_eq!(
        unsafe { pop_rt_ffi_callback_enter(context, 41, &raw mut bounded_environment) },
        0
    );
    assert_eq!(bounded_environment, 0xb0b0);
    assert_eq!(
        unsafe { pop_rt_leave_foreign(bounded, roots.as_mut_ptr(), 1) },
        1
    );

    let foreign = unsafe { pop_rt_enter_foreign(42, roots.as_mut_ptr(), 1, 0) };
    assert_ne!(foreign, 0);
    let callback = unsafe { pop_rt_ffi_callback_enter(context, 41, &raw mut callback_environment) };
    assert_ne!(callback, 0);
    assert_eq!(callback_environment, environment);

    let mut unchanged = 0xcccc;
    assert_eq!(
        unsafe { pop_rt_ffi_callback_enter(context, 41, &raw mut unchanged) },
        0
    );
    assert_eq!(unchanged, 0xcccc);
    assert_eq!(pop_rt_ffi_callback_close(registration, context, 41), 0);
    assert_eq!(pop_rt_ffi_callback_leave(callback), 1);
    assert_eq!(pop_rt_ffi_callback_leave(callback), 0);
    assert_eq!(
        unsafe { pop_rt_leave_foreign(foreign, roots.as_mut_ptr(), 1) },
        1
    );

    assert_eq!(pop_rt_ffi_callback_close(registration, context, 42), 0);
    assert_eq!(pop_rt_ffi_callback_close(registration, context, 41), 1);
    assert_eq!(pop_rt_ffi_callback_close(registration, context, 41), 0);
    let mut stale = 0xdddd;
    assert_eq!(
        unsafe { pop_rt_ffi_callback_enter(context, 41, &raw mut stale) },
        0
    );
    assert_eq!(stale, 0xdddd);
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);
}

#[test]
#[allow(unsafe_code)]
fn callback_open_is_failure_atomic_for_invalid_contracts() {
    let binding = pop_rt_attach_managed_thread(1);
    assert_ne!(binding, 0);
    let environment = pop_rt_allocate_object(0);
    assert_ne!(environment, 0);
    for (site, scheduler, lifetime, thread) in
        [(0, 1, 0, 0), (1, 0, 0, 0), (1, 1, 2, 0), (1, 1, 0, 2)]
    {
        let mut context = 0xeeee;
        assert_eq!(
            unsafe {
                pop_rt_ffi_callback_open(
                    environment,
                    site,
                    scheduler,
                    lifetime,
                    thread,
                    &raw mut context,
                )
            },
            0
        );
        assert_eq!(context, 0xeeee);
    }
    let context = 0xffff;
    assert_eq!(
        unsafe { pop_rt_ffi_callback_open(environment, 1, 1, 0, 0, std::ptr::null_mut()) },
        0
    );
    assert_eq!(context, 0xffff);
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);
}

#[test]
#[allow(unsafe_code)]
fn attached_thread_callback_balances_its_private_binding() {
    let binding = pop_rt_attach_managed_thread(2);
    assert_ne!(binding, 0);
    let environment = pop_rt_allocate_object(0);
    assert_ne!(environment, 0);
    let mut context = 0;
    let registration =
        unsafe { pop_rt_ffi_callback_open(environment, 71, 2, 1, 1, &raw mut context) };
    assert_ne!(registration, 0);
    assert_ne!(context, 0);
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);

    let worker = std::thread::spawn(move || {
        let mut callback_environment = 0;
        let transition =
            unsafe { pop_rt_ffi_callback_enter(context, 71, &raw mut callback_environment) };
        assert_ne!(transition, 0);
        assert_eq!(callback_environment, environment);
        assert_eq!(pop_rt_ffi_callback_leave(transition), 1);

        let binding = pop_rt_attach_managed_thread(2);
        assert_ne!(binding, 0, "callback leave detached its private binding");
        assert_eq!(pop_rt_detach_managed_thread(binding), 1);
    });
    worker.join().expect("attached callback worker");

    let binding = pop_rt_attach_managed_thread(2);
    assert_ne!(binding, 0);
    assert_eq!(pop_rt_ffi_callback_close(registration, context, 71), 1);
    assert_eq!(pop_rt_detach_managed_thread(binding), 1);
}

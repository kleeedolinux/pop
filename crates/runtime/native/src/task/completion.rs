#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_group_wrap(group: u64, body: u64, completion_is_managed: u8) -> u64 {
    let token = native_task_registry()
        .lock()
        .ok()
        .and_then(|registry| {
            registry
                .groups
                .get(&group)
                .map(|group| group.lifecycle.cancellation_token().raw())
        })
        .unwrap_or(0);
    if token == 0 || body == 0 || completion_is_managed > 1 {
        return 0;
    }
    let frame = NativeTaskFrame::new(
        vec![0, group, body, 0, 0, u64::from(completion_is_managed)],
        SafePointId::new(0),
        vec![RootSlot::new(1), RootSlot::new(2)],
    );
    let Ok(frame) = frame else {
        return 0;
    };
    let frame = Box::into_raw(Box::new(frame)) as usize as u64;
    pop_rt_task_create(frame, poll_group_wrapper, token, completion_is_managed)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_completion_store(task: u64, completion: u64) -> u8 {
    {
        let Ok(mut registry) = native_task_registry().lock() else {
            return 0;
        };
        let Some(record) = registry.tasks.get_mut(&task) else {
            return 0;
        };
        if record.completion_stored {
            return 0;
        }
        record.completion_stored = true;
    }
    if crate::pop_rt_field_set(task, 1, completion) == 1 {
        return 1;
    }
    if let Ok(mut registry) = native_task_registry().lock()
        && let Some(record) = registry.tasks.get_mut(&task)
    {
        record.completion_stored = false;
    }
    0
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
/// Observes one task's exact terminal value or nonterminal status.
///
/// # Safety
///
/// `completion` must be writable for one `u64` for the duration of this call.
pub unsafe extern "C" fn pop_rt_task_await(task: u64, completion: *mut u64) -> u8 {
    if completion.is_null() {
        return NativeTaskStatus::Failure as u8;
    }
    let created = native_task_registry().lock().ok().and_then(|registry| {
        registry
            .tasks
            .get(&task)
            .map(|record| record.lifecycle.state())
    }) == Some(TaskState::Created);
    if created {
        let parent = CURRENT_NATIVE_TASK.get().map_or(0, |current| current.0);
        if pop_rt_task_start_direct(task, parent) == 0 {
            return NativeTaskStatus::Failure as u8;
        }
    }
    if CURRENT_NATIVE_TASK.get().is_none() {
        let _ = native_task_scheduler().wait_until_idle(Duration::from_secs(5));
    }
    let (status, completion_stored) = {
        let Ok(mut registry) = native_task_registry().lock() else {
            return NativeTaskStatus::Failure as u8;
        };
        let Some(record) = registry.tasks.get_mut(&task) else {
            return NativeTaskStatus::Failure as u8;
        };
        let status = task_status(record);
        if !record.lifecycle.state().terminal()
            && let Some((_, waiter)) = CURRENT_NATIVE_TASK.get()
        {
            record.waiter = Some(waiter);
        }
        (status, record.completion_stored)
    };
    if status == NativeTaskStatus::Completed {
        if !completion_stored {
            return NativeTaskStatus::Failure as u8;
        }
        let value = crate::pop_rt_field_get(task, 1);
        unsafe { completion.write(value) };
    }
    status as u8
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_release(task: u64) -> u8 {
    let scheduler_task = {
        let Ok(mut registry) = native_task_registry().lock() else {
            return 0;
        };
        let Some(record) = registry.tasks.get(&task) else {
            return 0;
        };
        if !record.lifecycle.state().terminal() {
            return 0;
        }
        let scheduler_task = record.scheduler_task;
        registry.tasks.remove(&task);
        scheduler_task
    };
    if let Some(scheduler_task) = scheduler_task {
        let _ = native_task_scheduler().release_terminal_task(scheduler_task);
    }
    1
}

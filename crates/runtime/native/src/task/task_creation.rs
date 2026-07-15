#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_create(
    frame: u64,
    callback: NativeTaskPollCallback,
    cancellation_token: u64,
    completion_is_managed: u8,
) -> u64 {
    if frame == 0 || completion_is_managed > 1 {
        return 0;
    }
    let Some(frame) = native_task_frame_pointer(frame) else {
        return 0;
    };
    // SAFETY: Task creation consumes the unique cold frame handle.
    let frame = unsafe { *Box::from_raw(frame) };
    let Ok(publication) = frame.publication() else {
        return 0;
    };
    let mut cold_roots = Vec::new();
    for (_, reference) in publication.root_values() {
        let Some(reference) = reference else {
            continue;
        };
        let root = crate::pop_rt_retain_root(reference.raw());
        if root == 0 {
            for retained in cold_roots {
                let _ = crate::pop_rt_release_root(retained);
            }
            return 0;
        }
        cold_roots.push(root);
    }
    let completion_roots = if completion_is_managed == 1 {
        vec![0_u32, 1_u32]
    } else {
        vec![1_u32]
    };
    let handle = unsafe {
        crate::pop_rt_allocate_mapped_object(
            2,
            completion_roots.as_ptr(),
            completion_roots.len() as u64,
        )
    };
    if handle == 0 {
        for root in cold_roots {
            let _ = crate::pop_rt_release_root(root);
        }
        return 0;
    }
    if crate::pop_rt_field_set(handle, 2, cancellation_token) == 0 {
        for root in cold_roots {
            let _ = crate::pop_rt_release_root(root);
        }
        return 0;
    }
    let mut lifecycle = TaskLifecycle::created(TaskId::new(handle));
    if cancellation_token != 0
        && lifecycle
            .bind_cancellation_token(CancellationTokenId::new(cancellation_token))
            .is_err()
    {
        for root in cold_roots {
            let _ = crate::pop_rt_release_root(root);
        }
        return 0;
    }
    let Ok(mut registry) = native_task_registry().lock() else {
        for root in cold_roots {
            let _ = crate::pop_rt_release_root(root);
        }
        return 0;
    };
    registry.tasks.insert(
        handle,
        NativeAbiTask {
            lifecycle,
            frame: Some(frame),
            cold_roots,
            callback,
            scheduler_task: None,
            completion_stored: false,
            waiter: None,
        },
    );
    handle
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_start_direct(task: u64, parent: u64) -> u8 {
    let owner = TaskOwner::DirectAwait {
        parent: (parent != 0).then(|| TaskId::new(parent)),
    };
    {
        let Ok(mut registry) = native_task_registry().lock() else {
            return 0;
        };
        let Some(record) = registry.tasks.get_mut(&task) else {
            return 0;
        };
        if record.lifecycle.start(owner).is_err() {
            return 0;
        }
    }
    if schedule_started_task(task) {
        return 1;
    }
    let _ = native_task_registry()
        .lock()
        .ok()
        .and_then(|mut registry| {
            registry
                .tasks
                .get_mut(&task)?
                .lifecycle
                .rollback_unpolled_start(owner, false)
                .ok()
        })
        .is_some();
    0
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_group_create(cancellation_token: u64) -> u64 {
    if cancellation_token == 0 {
        return 0;
    }
    let roots = [0_u32];
    let handle = unsafe { crate::pop_rt_allocate_mapped_object(1, roots.as_ptr(), 1) };
    if handle == 0 {
        return 0;
    }
    if crate::pop_rt_field_set(handle, 1, cancellation_token) == 0 {
        return 0;
    }
    let Ok(mut registry) = native_task_registry().lock() else {
        return 0;
    };
    registry.groups.insert(
        handle,
        NativeAbiTaskGroup {
            lifecycle: TaskGroupLifecycle::open(
                TaskGroupId::new(handle),
                CancellationTokenId::new(cancellation_token),
            ),
            children: BTreeMap::new(),
        },
    );
    handle
}

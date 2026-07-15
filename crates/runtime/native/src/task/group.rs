#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_start_group(group: u64, task: u64) -> u8 {
    let child_root = crate::pop_rt_retain_root(task);
    if child_root == 0 {
        return 0;
    }
    let token_was_bound;
    let started = {
        let Ok(mut registry) = native_task_registry().lock() else {
            let _ = crate::pop_rt_release_root(child_root);
            return 0;
        };
        let Some(mut task_record) = registry.tasks.remove(&task) else {
            drop(registry);
            let _ = crate::pop_rt_release_root(child_root);
            return 0;
        };
        token_was_bound = task_record.lifecycle.cancellation_token().is_some();
        let started = registry.groups.get_mut(&group).is_some_and(|group_record| {
            if group_record
                .lifecycle
                .start_child(&mut task_record.lifecycle)
                .is_err()
            {
                return false;
            }
            group_record
                .children
                .insert(task_record.lifecycle.id(), (task, child_root));
            true
        });
        registry.tasks.insert(task, task_record);
        started
    };
    if !started {
        let _ = crate::pop_rt_release_root(child_root);
        return 0;
    }
    if schedule_started_task(task) {
        return 1;
    }
    let Ok(mut registry) = native_task_registry().lock() else {
        return 0;
    };
    let Some(mut task_record) = registry.tasks.remove(&task) else {
        return 0;
    };
    let rolled_back = registry.groups.get_mut(&group).is_some_and(|group_record| {
        if group_record
            .lifecycle
            .rollback_unpolled_child(&mut task_record.lifecycle, !token_was_bound)
            .is_err()
        {
            return false;
        }
        group_record.children.remove(&TaskId::new(task));
        true
    });
    registry.tasks.insert(task, task_record);
    drop(registry);
    let _ = crate::pop_rt_release_root(child_root);
    let _ = rolled_back;
    0
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_group_close(group: u64, exit: u8) -> u8 {
    let exit = match exit {
        0 => TaskGroupExit::BodyCompleted,
        1 => TaskGroupExit::BodyFailed,
        2 => TaskGroupExit::Cancelled,
        3 => TaskGroupExit::BodyPanicked,
        _ => return 0,
    };
    let scheduler_tasks = {
        let Ok(mut registry) = native_task_registry().lock() else {
            return 0;
        };
        let Some(mut group_record) = registry.groups.remove(&group) else {
            return 0;
        };
        let Ok(children) = group_record.lifecycle.begin_close(exit) else {
            registry.groups.insert(group, group_record);
            return 0;
        };
        let token = group_record.lifecycle.cancellation_token();
        let mut scheduler_tasks = Vec::new();
        for child in children {
            let Some((handle, _)) = group_record.children.get(&child).copied() else {
                registry.groups.insert(group, group_record);
                return 0;
            };
            let Some(task) = registry.tasks.get_mut(&handle) else {
                registry.groups.insert(group, group_record);
                return 0;
            };
            if !task.lifecycle.state().terminal() {
                let _ = task.lifecycle.request_cancellation(token);
                if let Some(scheduler_task) = task.scheduler_task {
                    scheduler_tasks.push(scheduler_task);
                }
            }
        }
        registry.groups.insert(group, group_record);
        scheduler_tasks
    };
    for task in scheduler_tasks {
        let _ = native_task_scheduler().request_cancellation(task);
    }
    1
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_group_join(group: u64) -> u8 {
    if CURRENT_NATIVE_TASK.get().is_none() {
        let _ = native_task_scheduler().wait_until_idle(Duration::from_secs(5));
    }
    let (status, cancellations, releases, root_releases) = {
        let Ok(mut registry) = native_task_registry().lock() else {
            return NativeTaskStatus::Failure as u8;
        };
        let Some(mut group_record) = registry.groups.remove(&group) else {
            return NativeTaskStatus::Failure as u8;
        };
        let children = group_record.lifecycle.unfinished_children();
        let mut pending = false;
        let mut releases = Vec::new();
        let mut root_releases = Vec::new();
        for child in children {
            let Some((handle, child_root)) = group_record.children.get(&child).copied() else {
                registry.groups.insert(group, group_record);
                return NativeTaskStatus::Failure as u8;
            };
            let Some(task) = registry.tasks.get(&handle) else {
                registry.groups.insert(group, group_record);
                return NativeTaskStatus::Failure as u8;
            };
            if task.lifecycle.state().terminal() {
                let lifecycle = task.lifecycle;
                if group_record.lifecycle.join_child(&lifecycle).is_err() {
                    registry.groups.insert(group, group_record);
                    return NativeTaskStatus::Failure as u8;
                }
                let Some(removed) = registry.tasks.remove(&handle) else {
                    registry.groups.insert(group, group_record);
                    return NativeTaskStatus::Failure as u8;
                };
                group_record.children.remove(&child);
                root_releases.push(child_root);
                if let Some(scheduler_task) = removed.scheduler_task {
                    releases.push(scheduler_task);
                }
            } else {
                pending = true;
                if let Some((_, waiter)) = CURRENT_NATIVE_TASK.get()
                    && let Some(task) = registry.tasks.get_mut(&handle)
                {
                    task.waiter = Some(waiter);
                }
            }
        }
        let cancellations = if matches!(
            group_record.lifecycle.state(),
            pop_runtime_interface::TaskGroupState::Closing(TaskGroupExit::ChildPanicked(_))
        ) {
            let token = group_record.lifecycle.cancellation_token();
            group_record
                .lifecycle
                .unfinished_children()
                .into_iter()
                .filter_map(|child| {
                    let (handle, _) = group_record.children.get(&child)?;
                    let task = registry.tasks.get_mut(handle)?;
                    let _ = task.lifecycle.request_cancellation(token);
                    task.scheduler_task
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if pending {
            registry.groups.insert(group, group_record);
            (
                NativeTaskStatus::Pending,
                cancellations,
                releases,
                root_releases,
            )
        } else {
            let status = match group_record.lifecycle.complete_close() {
                Ok(TaskGroupExit::BodyCompleted) => NativeTaskStatus::Completed,
                Ok(TaskGroupExit::Cancelled) => NativeTaskStatus::Cancelled,
                Ok(TaskGroupExit::BodyPanicked | TaskGroupExit::ChildPanicked(_)) => {
                    NativeTaskStatus::Panicked
                }
                Ok(TaskGroupExit::BodyFailed) | Err(_) => NativeTaskStatus::Failure,
            };
            (status, cancellations, releases, root_releases)
        }
    };
    for task in cancellations {
        let _ = native_task_scheduler().request_cancellation(task);
    }
    for task in releases {
        let _ = native_task_scheduler().release_terminal_task(task);
    }
    for root in root_releases {
        let _ = crate::pop_rt_release_root(root);
    }
    status as u8
}

#[allow(unsafe_code)]
extern "C" fn poll_group_wrapper(task: u64, frame: u64, _cancelled: u8) -> u8 {
    let Some(frame) = native_task_frame_pointer(frame) else {
        return NativeTaskStatus::Panicked as u8;
    };
    let Some(frame) = (unsafe { frame.as_mut() }) else {
        return NativeTaskStatus::Panicked as u8;
    };
    let Ok(state) = frame.slot(0) else {
        return NativeTaskStatus::Panicked as u8;
    };
    let Ok(group) = frame.slot(1) else {
        return NativeTaskStatus::Panicked as u8;
    };
    let Ok(body) = frame.slot(2) else {
        return NativeTaskStatus::Panicked as u8;
    };
    if state == 0 {
        let mut completion = 0;
        let status = unsafe { pop_rt_task_await(body, &raw mut completion) };
        let Some(status) = NativeTaskStatus::from_raw(status) else {
            return NativeTaskStatus::Panicked as u8;
        };
        if matches!(status, NativeTaskStatus::Ready | NativeTaskStatus::Pending) {
            return NativeTaskStatus::Pending as u8;
        }
        let exit = match status {
            NativeTaskStatus::Completed => 0,
            NativeTaskStatus::Cancelled => 2,
            NativeTaskStatus::Panicked => 3,
            NativeTaskStatus::Failure => 1,
            NativeTaskStatus::Ready | NativeTaskStatus::Pending => unreachable!(),
        };
        if frame.set_slot(3, completion).is_err()
            || frame.set_slot(4, status as u64).is_err()
            || frame.set_slot(0, 1).is_err()
            || frame
                .set_live_frame(
                    SafePointId::new(1),
                    if frame.slot(5).ok() == Some(1) && status == NativeTaskStatus::Completed {
                        vec![RootSlot::new(1), RootSlot::new(3)]
                    } else {
                        vec![RootSlot::new(1)]
                    },
                )
                .is_err()
            || pop_rt_task_group_close(group, exit) == 0
            || pop_rt_task_release(body) == 0
        {
            return NativeTaskStatus::Panicked as u8;
        }
    }
    let joined = pop_rt_task_group_join(group);
    if joined == NativeTaskStatus::Pending as u8 {
        return joined;
    }
    let Some(joined) = NativeTaskStatus::from_raw(joined) else {
        return NativeTaskStatus::Panicked as u8;
    };
    if joined != NativeTaskStatus::Completed {
        return joined as u8;
    }
    let status = frame
        .slot(4)
        .ok()
        .and_then(|status| u8::try_from(status).ok())
        .and_then(NativeTaskStatus::from_raw)
        .unwrap_or(NativeTaskStatus::Panicked);
    if status == NativeTaskStatus::Completed {
        let Ok(completion) = frame.slot(3) else {
            return NativeTaskStatus::Panicked as u8;
        };
        if pop_rt_task_completion_store(task, completion) == 0 {
            return NativeTaskStatus::Panicked as u8;
        }
    }
    status as u8
}

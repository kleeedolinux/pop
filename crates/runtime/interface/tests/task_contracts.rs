use pop_runtime_interface::{
    CancellationObservation, CancellationTokenId, TaskGroupExit, TaskGroupId, TaskGroupLifecycle,
    TaskGroupLifecycleError, TaskGroupState, TaskId, TaskLifecycle, TaskLifecycleError, TaskOwner,
    TaskPollCompletion, TaskState,
};

#[test]
fn cold_task_acquires_exactly_one_owner_and_repeated_start_fails_closed() {
    let mut task = TaskLifecycle::created(TaskId::new(7));
    let owner = TaskOwner::DirectAwait {
        parent: Some(TaskId::new(3)),
    };

    assert_eq!(task.state(), TaskState::Created);
    assert_eq!(task.owner(), None);
    assert_eq!(task.start(owner), Ok(()));
    assert_eq!(task.state(), TaskState::Ready);
    assert_eq!(task.owner(), Some(owner));
    assert_eq!(
        task.start(TaskOwner::Group(TaskGroupId::new(2))),
        Err(TaskLifecycleError::AlreadyStarted(TaskId::new(7)))
    );
    assert_eq!(task.owner(), Some(owner));
}

#[test]
fn rejected_scheduler_admission_rolls_back_an_unpolled_owner_atomically() {
    let task_id = TaskId::new(8);
    let owner = TaskOwner::DirectAwait { parent: None };
    let mut task = TaskLifecycle::created(task_id);
    task.start(owner).expect("provisional direct owner");
    task.rollback_unpolled_start(owner, false)
        .expect("failed admission restores a cold task");
    assert_eq!(task.state(), TaskState::Created);
    assert_eq!(task.owner(), None);

    let group_id = TaskGroupId::new(9);
    let mut group = TaskGroupLifecycle::open(group_id, CancellationTokenId::new(10));
    group
        .start_child(&mut task)
        .expect("provisional group owner");
    group
        .rollback_unpolled_child(&mut task, true)
        .expect("failed group admission restores both records");
    assert!(group.unfinished_children().is_empty());
    assert_eq!(task.state(), TaskState::Created);
    assert_eq!(task.owner(), None);
    assert_eq!(task.cancellation_token(), None);
}

#[test]
fn polling_and_terminal_completion_follow_the_closed_coroutine_state_machine() {
    let mut task = TaskLifecycle::created(TaskId::new(9));
    task.start(TaskOwner::DirectAwait { parent: None })
        .expect("host owns direct await");

    assert_eq!(task.begin_poll(), Ok(()));
    assert_eq!(task.state(), TaskState::Running);
    assert_eq!(task.finish_poll(TaskPollCompletion::Pending), Ok(()));
    assert_eq!(task.state(), TaskState::Suspended);
    assert_eq!(task.begin_poll(), Ok(()));
    assert_eq!(task.finish_poll(TaskPollCompletion::Completed), Ok(()));
    assert_eq!(task.state(), TaskState::Completed);
    assert!(task.completed());
    assert_eq!(
        task.begin_poll(),
        Err(TaskLifecycleError::Terminal(TaskId::new(9)))
    );
}

#[test]
fn cancellation_is_explicit_and_cleanup_masking_defers_observation() {
    let token = CancellationTokenId::new(5);
    let mut task = TaskLifecycle::created(TaskId::new(11));
    task.start(TaskOwner::Group(TaskGroupId::new(4)))
        .expect("group owns task");

    assert_eq!(task.cancellation_token(), None);
    task.bind_cancellation_token(token)
        .expect("explicit token binding");
    assert_eq!(task.cancellation_token(), Some(token));
    assert_eq!(
        task.cancellation_observation(false),
        CancellationObservation::Active
    );
    assert_eq!(task.request_cancellation(token), Ok(true));
    assert_eq!(task.request_cancellation(token), Ok(false));
    assert_eq!(
        task.cancellation_observation(true),
        CancellationObservation::Masked
    );
    assert_eq!(
        task.cancellation_observation(false),
        CancellationObservation::Requested
    );
    assert_eq!(
        task.request_cancellation(CancellationTokenId::new(6)),
        Err(TaskLifecycleError::CancellationTokenMismatch {
            expected: token,
            found: CancellationTokenId::new(6),
        })
    );
}

#[test]
fn task_group_takes_exact_ownership_and_closes_only_after_every_child_joins() {
    let group_id = TaskGroupId::new(17);
    let token = CancellationTokenId::new(23);
    let mut group = TaskGroupLifecycle::open(group_id, token);
    let mut first = TaskLifecycle::created(TaskId::new(31));
    let mut second = TaskLifecycle::created(TaskId::new(32));

    group.start_child(&mut first).expect("first child starts");
    group.start_child(&mut second).expect("second child starts");
    assert_eq!(first.owner(), Some(TaskOwner::Group(group_id)));
    assert_eq!(second.owner(), Some(TaskOwner::Group(group_id)));
    assert_eq!(first.cancellation_token(), Some(token));
    assert_eq!(
        group.unfinished_children(),
        [TaskId::new(31), TaskId::new(32)]
    );

    first.begin_poll().expect("first poll starts");
    first
        .finish_poll(TaskPollCompletion::Completed)
        .expect("first completes");
    group.join_child(&first).expect("terminal child joins");
    assert_eq!(
        group.join_child(&second),
        Err(TaskGroupLifecycleError::ChildNotTerminal(TaskId::new(32)))
    );

    assert_eq!(
        group.begin_close(TaskGroupExit::BodyCompleted),
        Ok(vec![TaskId::new(32)])
    );
    assert_eq!(
        group.state(),
        TaskGroupState::Closing(TaskGroupExit::BodyCompleted)
    );
    assert_eq!(
        group.complete_close(),
        Err(TaskGroupLifecycleError::ChildrenRemain(group_id))
    );
    second
        .request_cancellation(token)
        .expect("group requests cancellation through its token");
    second.begin_poll().expect("second poll starts");
    second
        .finish_poll(TaskPollCompletion::Cancelled)
        .expect("second finishes cleanup and cancels");
    group.join_child(&second).expect("cancelled child joins");
    assert_eq!(group.complete_close(), Ok(TaskGroupExit::BodyCompleted));
    assert_eq!(
        group.state(),
        TaskGroupState::Closed(TaskGroupExit::BodyCompleted)
    );
}

#[test]
fn child_panic_requires_sibling_cancellation_before_group_propagation() {
    let group_id = TaskGroupId::new(41);
    let token = CancellationTokenId::new(42);
    let mut group = TaskGroupLifecycle::open(group_id, token);
    let mut panicked = TaskLifecycle::created(TaskId::new(43));
    let mut sibling = TaskLifecycle::created(TaskId::new(44));
    group
        .start_child(&mut panicked)
        .expect("panic child starts");
    group.start_child(&mut sibling).expect("sibling starts");

    panicked.begin_poll().expect("panic child polls");
    panicked
        .finish_poll(TaskPollCompletion::Panicked)
        .expect("panic becomes a terminal task outcome");
    assert_eq!(
        group.begin_close(TaskGroupExit::BodyCompleted),
        Ok(vec![TaskId::new(43), TaskId::new(44)])
    );
    group.join_child(&panicked).expect("panic child joins");
    assert_eq!(
        group.state(),
        TaskGroupState::Closing(TaskGroupExit::ChildPanicked(TaskId::new(43)))
    );
    assert_eq!(
        group.complete_close(),
        Err(TaskGroupLifecycleError::ChildrenRemain(group_id))
    );

    sibling
        .request_cancellation(token)
        .expect("sibling cancellation is explicit");
    sibling.begin_poll().expect("sibling polls cleanup");
    sibling
        .finish_poll(TaskPollCompletion::Cancelled)
        .expect("sibling cleanup finishes");
    group.join_child(&sibling).expect("sibling joins");
    assert_eq!(
        group.complete_close(),
        Ok(TaskGroupExit::ChildPanicked(TaskId::new(43)))
    );
}

#[test]
fn closing_group_rejects_new_children_without_stealing_their_owner() {
    let group_id = TaskGroupId::new(51);
    let mut group = TaskGroupLifecycle::open(group_id, CancellationTokenId::new(52));
    assert_eq!(group.begin_close(TaskGroupExit::Cancelled), Ok(Vec::new()));
    let mut late = TaskLifecycle::created(TaskId::new(53));

    assert_eq!(
        group.start_child(&mut late),
        Err(TaskGroupLifecycleError::NotOpen(group_id))
    );
    assert_eq!(late.state(), TaskState::Created);
    assert_eq!(late.owner(), None);
}

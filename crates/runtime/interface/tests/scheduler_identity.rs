use pop_runtime_interface::{SchedulerId, TaskFrameRootId};

#[test]
fn scheduler_and_task_frame_root_identities_are_closed_plri_values() {
    let scheduler = SchedulerId::new(7);
    let task_roots = TaskFrameRootId::new(11);

    assert_eq!(scheduler.raw(), 7);
    assert_eq!(task_roots.raw(), 11);
    assert_ne!(
        std::any::TypeId::of::<SchedulerId>(),
        std::any::TypeId::of::<TaskFrameRootId>()
    );
}

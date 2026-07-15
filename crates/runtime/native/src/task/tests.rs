#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn never_polled(_task: u64, _frame: u64, _cancelled: u8) -> u8 {
        NativeTaskStatus::Panicked as u8
    }

    #[test]
    fn collection_prunes_an_unreachable_cold_task_and_its_external_roots() {
        let capture = crate::pop_rt_allocate_object(0);
        let frame =
            NativeTaskFrame::new(vec![capture], SafePointId::new(90), vec![RootSlot::new(0)])
                .expect("cold frame");
        let frame = Box::into_raw(Box::new(frame)) as usize as u64;
        let task = pop_rt_task_create(frame, never_polled, 0, 0);
        assert!(
            native_task_registry()
                .lock()
                .expect("task registry")
                .tasks
                .contains_key(&task)
        );

        assert!(crate::request_abi_collection());
        assert_eq!(crate::abi_safe_point(91, &[]), 1);
        assert!(
            !native_task_registry()
                .lock()
                .expect("task registry")
                .tasks
                .contains_key(&task)
        );
    }
}

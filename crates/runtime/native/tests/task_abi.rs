use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use pop_runtime_native::{
    abi_safe_point, pop_rt_allocate_object, pop_rt_cancel_source_create,
    pop_rt_cancel_source_release, pop_rt_cancel_source_token, pop_rt_cancel_token_release,
    pop_rt_release_root, pop_rt_retain_root, pop_rt_task_await, pop_rt_task_completion_store,
    pop_rt_task_create, pop_rt_task_frame_create, pop_rt_task_frame_load,
    pop_rt_task_frame_release, pop_rt_task_frame_set_live_map, pop_rt_task_frame_store,
    pop_rt_task_group_close, pop_rt_task_group_create, pop_rt_task_group_join, pop_rt_task_release,
    pop_rt_task_start_direct, pop_rt_task_start_group, request_abi_collection,
};
use pop_runtime_native_abi::NativeTaskStatus;

static POLLS: AtomicUsize = AtomicUsize::new(0);
static MANAGED_COMPLETION: AtomicU64 = AtomicU64::new(0);

fn task_abi_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("task ABI test lock")
}

#[allow(unsafe_code)]
extern "C" fn complete_scalar(task: u64, frame: u64, cancelled: u8) -> u8 {
    assert_eq!(cancelled, 0);
    POLLS.fetch_add(1, Ordering::SeqCst);
    let mut input = 0;
    // SAFETY: The scheduler callback receives the exact live frame and the
    // output points to writable storage for this call.
    assert_eq!(
        unsafe { pop_rt_task_frame_load(frame, 0, &raw mut input) },
        1
    );
    assert_eq!(pop_rt_task_completion_store(task, input + 1), 1);
    NativeTaskStatus::Completed as u8
}

extern "C" fn complete_group_child(task: u64, _frame: u64, _cancelled: u8) -> u8 {
    assert_eq!(pop_rt_task_completion_store(task, 9), 1);
    NativeTaskStatus::Completed as u8
}

extern "C" fn complete_managed(task: u64, _frame: u64, _cancelled: u8) -> u8 {
    assert_eq!(
        pop_rt_task_completion_store(task, MANAGED_COMPLETION.load(Ordering::SeqCst)),
        1
    );
    NativeTaskStatus::Completed as u8
}

#[allow(unsafe_code)]
extern "C" fn complete_managed_capture(task: u64, frame: u64, _cancelled: u8) -> u8 {
    let mut capture = 0;
    if unsafe { pop_rt_task_frame_load(frame, 0, &raw mut capture) } == 0
        || pop_rt_task_completion_store(task, capture) == 0
    {
        return NativeTaskStatus::Panicked as u8;
    }
    NativeTaskStatus::Completed as u8
}

#[test]
#[allow(unsafe_code)]
fn compiler_abi_task_stays_cold_then_executes_on_the_native_scheduler() {
    let _guard = task_abi_test_lock();
    POLLS.store(0, Ordering::SeqCst);
    let roots = [];
    // SAFETY: The empty root slice is valid for its declared count.
    let frame = unsafe { pop_rt_task_frame_create(1, 7, roots.as_ptr(), 0) };
    assert_ne!(frame, 0);
    assert_eq!(unsafe { pop_rt_task_frame_store(frame, 0, 41) }, 1);
    let task = pop_rt_task_create(frame, complete_scalar, 0, 0);
    assert_ne!(task, 0);
    assert_eq!(POLLS.load(Ordering::SeqCst), 0, "creation must stay cold");

    assert_eq!(pop_rt_task_start_direct(task, 0), 1);
    let mut completion = 0;
    assert_eq!(
        unsafe { pop_rt_task_await(task, &raw mut completion) },
        NativeTaskStatus::Completed as u8
    );
    assert_eq!(completion, 42);
    assert_eq!(POLLS.load(Ordering::SeqCst), 1);
    assert_eq!(pop_rt_task_start_direct(task, 0), 0, "start is exact once");
    assert_eq!(pop_rt_task_release(task), 1);
}

#[test]
#[allow(unsafe_code)]
fn managed_completion_is_traced_by_the_retained_task_object() {
    let _guard = task_abi_test_lock();
    let completion = pop_rt_allocate_object(0);
    MANAGED_COMPLETION.store(completion, Ordering::SeqCst);
    let frame = unsafe { pop_rt_task_frame_create(0, 30, std::ptr::null(), 0) };
    let task = pop_rt_task_create(frame, complete_managed, 0, 1);
    assert_ne!(task, 0);
    let task_root = pop_rt_retain_root(task);
    assert_ne!(task_root, 0);
    assert_eq!(pop_rt_task_start_direct(task, 0), 1);
    let mut observed = 0;
    assert_eq!(
        unsafe { pop_rt_task_await(task, &raw mut observed) },
        NativeTaskStatus::Completed as u8
    );
    assert_eq!(observed, completion);

    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(31, &[]), 1);
    observed = 0;
    assert_eq!(
        unsafe { pop_rt_task_await(task, &raw mut observed) },
        NativeTaskStatus::Completed as u8
    );
    assert_eq!(observed, completion);
    assert_eq!(pop_rt_release_root(task_root), 1);
    assert_eq!(pop_rt_task_release(task), 1);
}

#[test]
#[allow(unsafe_code)]
fn cold_task_frame_retains_its_precise_managed_captures() {
    let _guard = task_abi_test_lock();
    let capture = pop_rt_allocate_object(0);
    let roots = [0_u32];
    let frame = unsafe { pop_rt_task_frame_create(1, 32, roots.as_ptr(), 1) };
    assert_eq!(unsafe { pop_rt_task_frame_store(frame, 0, capture) }, 1);
    let task = pop_rt_task_create(frame, complete_managed_capture, 0, 1);
    assert_ne!(task, 0);

    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(33, &[task]), 1);
    let mut observed = 0;
    assert_eq!(
        unsafe { pop_rt_task_await(task, &raw mut observed) },
        NativeTaskStatus::Completed as u8
    );
    assert_eq!(observed, capture);
    assert_eq!(pop_rt_task_release(task), 1);
}

#[test]
#[allow(unsafe_code)]
fn frame_live_map_replacement_is_failure_atomic() {
    let _guard = task_abi_test_lock();
    let initial_roots = [1_u32];
    let frame = unsafe {
        pop_rt_task_frame_create(2, 10, initial_roots.as_ptr(), initial_roots.len() as u64)
    };
    assert_ne!(frame, 0);
    let invalid = [2_u32];
    assert_eq!(
        unsafe {
            pop_rt_task_frame_set_live_map(frame, 11, invalid.as_ptr(), invalid.len() as u64)
        },
        0
    );
    let mut retained = 0;
    assert_eq!(
        unsafe { pop_rt_task_frame_load(frame, 1, &raw mut retained) },
        1
    );
    assert_eq!(pop_rt_task_frame_release(frame), 1);
}

#[test]
#[allow(unsafe_code)]
fn native_group_owns_each_child_once_and_joins_before_completion() {
    let _guard = task_abi_test_lock();
    let source = pop_rt_cancel_source_create();
    let token = pop_rt_cancel_source_token(source);
    let group = pop_rt_task_group_create(token);
    assert_ne!(group, 0);
    let frame = unsafe { pop_rt_task_frame_create(0, 20, std::ptr::null(), 0) };
    let task = pop_rt_task_create(frame, complete_group_child, 0, 0);
    assert_eq!(pop_rt_task_start_group(group, task), 1);
    assert_eq!(pop_rt_task_start_group(group, task), 0);
    assert_eq!(pop_rt_task_group_close(group, 0), 1);
    assert_eq!(
        pop_rt_task_group_join(group),
        NativeTaskStatus::Completed as u8
    );
    assert_eq!(
        pop_rt_task_release(task),
        0,
        "joining a group releases its owned child"
    );
    assert_eq!(pop_rt_cancel_source_release(source), 1);
    assert_eq!(pop_rt_cancel_token_release(token), 1);
}

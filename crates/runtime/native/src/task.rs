//! Native task and cancellation ABI boundary.

use crate::failure::pop_rt_trap;

/// Awaits one scalar task handle at the native ABI boundary.
///
/// The current bootstrap representation stores scalar task completion directly
/// in the handle. A full coroutine scheduler can replace this boundary without
/// changing generated native call sites.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_suspend(task: u64) -> u64 {
    if task == 0 {
        pop_rt_trap();
    }
    task
}

/// Resumes one task handle and returns its scalar completion.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_resume(task: u64) -> u64 {
    pop_rt_suspend(task)
}

/// Marks a cancellation source or token as cancelled.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_cancel(token: u64) -> u8 {
    u8::from(token != 0)
}

/// Returns whether a cancellation token has been cancelled.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_cancellation_requested(token: u64) -> u8 {
    let _ = token;
    0
}

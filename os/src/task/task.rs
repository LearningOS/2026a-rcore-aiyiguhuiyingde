//! Types related to task management

use super::TaskContext;
use crate::config::SYSCALL_COUNT_SLOTS;

/// The task control block (TCB) of a task.
#[derive(Copy, Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// Per-task histogram of syscall invocations, indexed by `syscall_id`.
    ///
    /// Design note: we deliberately keep this *inside* the TCB instead of in a
    /// side table. The TCB is plain data owned by `TASK_MANAGER`, so reading or
    /// bumping a counter never needs heap allocation and stays valid after the
    /// task exits (that is what makes `count_syscall(SYSCALL_EXIT)` observable).
    /// Slots for ids nobody ever calls are just wasted static bytes, which is
    /// cheap here because ch3 apps are loaded into fixed regions anyway.
    pub syscall_counts: [isize; SYSCALL_COUNT_SLOTS],
}

impl TaskControlBlock {
    /// A freshly created (not yet scheduled) TCB with an all-zero histogram.
    pub fn new(task_cx: TaskContext) -> Self {
        Self {
            task_status: TaskStatus::UnInit,
            task_cx,
            syscall_counts: [0; SYSCALL_COUNT_SLOTS],
        }
    }

    /// Record one invocation of `syscall_id`; silently ignore out-of-range ids
    /// rather than panicking, since a bogus id must not kill the tracer itself.
    pub fn record_syscall(&mut self, syscall_id: usize) {
        if syscall_id < self.syscall_counts.len() {
            self.syscall_counts[syscall_id] += 1;
        }
    }

    /// Read the recorded number of invocations of `syscall_id`
    /// (`None` means "this id is not tracked", mirroring the `-1` error code).
    pub fn get_syscall_count(&self, syscall_id: usize) -> Option<isize> {
        self.syscall_counts.get(syscall_id).copied()
    }
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}

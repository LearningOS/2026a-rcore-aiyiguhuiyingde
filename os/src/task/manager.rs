//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// The ready queue, served in order of virtual time spent.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take the task that has been charged the least virtual time.
    ///
    /// Every task carries a `pass` and a `stride`, the price of one dispatch.
    /// Taking the smallest `pass` and billing it straight away is stride
    /// scheduling measured in dispatches instead of ticks: a task that has
    /// asked for priority `p` pays `1/p` of the charge, so its pass climbs
    /// `p` times more slowly and it is picked roughly `p` times as often,
    /// while everyone else keeps making progress.
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let mut best: Option<(usize, usize)> = None;
        for (idx, task) in self.ready_queue.iter().enumerate() {
            let pass = task.inner_exclusive_access().pass;
            if best.map_or(true, |(_, lowest)| pass < lowest) {
                best = Some((idx, pass));
            }
        }
        let (idx, _) = best?;
        let task = self.ready_queue.remove(idx).unwrap();
        let stride = task.inner_exclusive_access().stride;
        task.inner_exclusive_access().pass += stride;
        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}

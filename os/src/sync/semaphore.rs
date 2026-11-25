//! Semaphore

use crate::sync::UPSafeCell;
use crate::task::{
    block_current_and_run_next, current_process, current_task, grant, hand_over, release,
    request_would_deadlock, set_waiting, wakeup_task, ResourceId, TaskControlBlock,
};
use alloc::{collections::VecDeque, sync::Arc};

/// semaphore structure
pub struct Semaphore {
    /// semaphore inner
    pub inner: UPSafeCell<SemaphoreInner>,
}

pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Semaphore {
    /// Create a new semaphore
    pub fn new(res_count: usize) -> Self {
        trace!("kernel: Semaphore::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// up operation of semaphore
    pub fn up(&self, sem_id: usize) {
        trace!("kernel: Semaphore::up");
        let mut inner = self.inner.exclusive_access();
        inner.count += 1;
        let next = if inner.count <= 0 {
            inner.wait_queue.pop_front()
        } else {
            None
        };
        drop(inner);
        if let Some(task) = next {
            hand_over(&task, ResourceId::Semaphore(sem_id));
            wakeup_task(task);
        } else {
            release(&current_task().unwrap(), ResourceId::Semaphore(sem_id));
        }
    }

    /// down operation of semaphore
    pub fn down(&self, sem_id: usize) -> bool {
        trace!("kernel: Semaphore::down");
        let resource = ResourceId::Semaphore(sem_id);
        let mut inner = self.inner.exclusive_access();
        if inner.count > 0 {
            inner.count -= 1;
            drop(inner);
            grant(&current_task().unwrap(), resource);
            return true;
        }
        drop(inner);
        let process = current_process();
        if request_would_deadlock(&process, resource) {
            return false;
        }
        let task = current_task().unwrap();
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        inner.wait_queue.push_back(Arc::clone(&task));
        drop(inner);
        set_waiting(&task, resource);
        block_current_and_run_next();
        true
    }
}

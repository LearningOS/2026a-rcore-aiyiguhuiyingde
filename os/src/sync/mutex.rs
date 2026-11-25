//! Mutex (spin-like and blocking(sleep))

use super::UPSafeCell;
use crate::task::TaskControlBlock;
use crate::task::{block_current_and_run_next, suspend_current_and_run_next};
use crate::task::{
    current_process, current_task, grant, hand_over, release, request_would_deadlock, set_waiting,
    wakeup_task, ResourceId,
};
use alloc::{collections::VecDeque, sync::Arc};

/// Mutex trait
pub trait Mutex: Sync + Send {
    /// Lock the mutex
    fn lock(&self) -> bool;
    /// Unlock the mutex
    fn unlock(&self);
}

/// Spinlock Mutex struct
pub struct MutexSpin {
    locked: UPSafeCell<bool>,
}

impl MutexSpin {
    /// Create a new spinlock mutex
    pub fn new() -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
        }
    }
}

impl Mutex for MutexSpin {
    /// Lock the spinlock mutex
    fn lock(&self) -> bool {
        trace!("kernel: MutexSpin::lock");
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                drop(locked);
                suspend_current_and_run_next();
                continue;
            } else {
                *locked = true;
                return true;
            }
        }
    }

    fn unlock(&self) {
        trace!("kernel: MutexSpin::unlock");
        let mut locked = self.locked.exclusive_access();
        *locked = false;
    }
}

/// Blocking Mutex struct
pub struct MutexBlocking {
    slot: usize,
    inner: UPSafeCell<MutexBlockingInner>,
}

pub struct MutexBlockingInner {
    owner: Option<usize>,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl MutexBlocking {
    /// Create a new blocking mutex
    pub fn new(slot: usize) -> Self {
        trace!("kernel: MutexBlocking::new");
        Self {
            slot,
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    owner: None,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }
}

impl Mutex for MutexBlocking {
    /// lock the blocking mutex
    fn lock(&self) -> bool {
        trace!("kernel: MutexBlocking::lock");
        let resource = ResourceId::Mutex(self.slot);
        let task = current_task().unwrap();
        let tid = task.inner_exclusive_access().res.as_ref().unwrap().tid;
        let mut mutex_inner = self.inner.exclusive_access();
        if mutex_inner.owner.is_none() {
            mutex_inner.owner = Some(tid);
            drop(mutex_inner);
            grant(&task, resource);
            return true;
        }
        drop(mutex_inner);
        if request_would_deadlock(&current_process(), resource) {
            return false;
        }
        let mut mutex_inner = self.inner.exclusive_access();
        mutex_inner.wait_queue.push_back(Arc::clone(&task));
        drop(mutex_inner);
        set_waiting(&task, resource);
        block_current_and_run_next();
        true
    }

    /// unlock the blocking mutex
    fn unlock(&self) {
        trace!("kernel: MutexBlocking::unlock");
        let resource = ResourceId::Mutex(self.slot);
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.owner.is_some());
        if let Some(waking_task) = mutex_inner.wait_queue.pop_front() {
            let tid = waking_task.inner_exclusive_access().res.as_ref().unwrap().tid;
            mutex_inner.owner = Some(tid);
            drop(mutex_inner);
            hand_over(&waking_task, resource);
            wakeup_task(waking_task);
        } else {
            mutex_inner.owner = None;
            drop(mutex_inner);
            release(&current_task().unwrap(), resource);
        }
    }
}

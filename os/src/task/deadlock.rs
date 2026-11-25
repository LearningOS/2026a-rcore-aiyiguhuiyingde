use super::{current_task, ProcessControlBlock, TaskControlBlock};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Identity of a synchronisation object inside one process.
///
/// `seq` is the slot index handed back to user space, so a semaphore that was
/// created with `res_count > 1` still counts as a single node in the wait-for
/// graph: the graph asks "can *this* resource ever be released?", not "how many
/// copies are left".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceId {
    /// A counting semaphore created by `sys_semaphore_create`.
    Semaphore(usize),
    /// A blocking mutex created by `sys_mutex_create(true)`.
    Mutex(usize),
}

/// One thread's edge in the snapshot the search walks.
///
/// `blocked_on` is `None` for a thread that is running or sleeping: such a
/// thread is a sink, so anything waiting behind it can still make progress.
struct ThreadSnapshot {
    tid: usize,
    held: Vec<ResourceId>,
    blocked_on: Option<ResourceId>,
}

/// Turn resource tracking on or off for `process`.
///
/// While it is off no bookkeeping happens, which keeps `sys_mutex_lock` on the
/// plain spin-wait path the rest of the test suite expects.
pub fn set_detection(process: &Arc<ProcessControlBlock>, enabled: bool) {
    process.inner_exclusive_access().deadlock_detect = enabled;
}

pub fn detection_enabled(process: &Arc<ProcessControlBlock>) -> bool {
    process.inner_exclusive_access().deadlock_detect
}

/// Would granting `resource` to the calling thread close a wait-for cycle?
///
/// The graph is read as a snapshot so no lock is held while we recurse. The
/// question is whether *someone other than the requester* can still finish:
/// we look for a holder of `resource` that is not itself stuck behind us.
/// `path` records resources already visited so a cycle cannot loop forever.
pub fn request_would_deadlock(process: &Arc<ProcessControlBlock>, resource: ResourceId) -> bool {
    if !detection_enabled(process) {
        return false;
    }
    let current = current_task().unwrap();
    let requester = current.inner_exclusive_access().res.as_ref().unwrap().tid;
    let tasks = {
        let inner = process.inner_exclusive_access();
        inner
            .tasks
            .iter()
            .filter_map(|task| task.clone())
            .collect::<Vec<_>>()
    };
    let snapshots = tasks
        .into_iter()
        .filter_map(|task| {
            let inner = task.inner_exclusive_access();
            let tid = inner.res.as_ref()?.tid;
            Some(ThreadSnapshot {
                tid,
                held: inner.held_resources.clone(),
                blocked_on: inner.blocked_on,
            })
        })
        .collect::<Vec<_>>();
    !can_release(
        resource,
        requester,
        &snapshots,
        &mut Vec::<ResourceId>::new(),
    )
}

fn can_release(
    resource: ResourceId,
    requester: usize,
    snapshots: &[ThreadSnapshot],
    path: &mut Vec<ResourceId>,
) -> bool {
    if path.contains(&resource) {
        return false;
    }
    path.push(resource);
    for holder in snapshots
        .iter()
        .filter(|thread| thread.tid != requester && thread.held.contains(&resource))
    {
        let progress = match holder.blocked_on {
            Some(next) => can_release(next, requester, snapshots, path),
            None => true,
        };
        if progress {
            path.pop();
            return true;
        }
    }
    path.pop();
    false
}

/// Record that `task` now owns `resource` (count down, mutex taken).
pub fn grant(task: &Arc<TaskControlBlock>, resource: ResourceId) {
    task.inner_exclusive_access()
        .held_resources
        .push(resource);
}

/// Forget one ownership of `resource` by `task` (count up, mutex released).
///
/// Only the innermost ownership is dropped, so a thread holding the same
/// semaphore twice still appears as a holder after a single `up`.
pub fn release(task: &Arc<TaskControlBlock>, resource: ResourceId) {
    let mut inner = task.inner_exclusive_access();
    if let Some(index) = inner
        .held_resources
        .iter()
        .position(|held| *held == resource)
    {
        inner.held_resources.remove(index);
    }
}

/// A waiter finally receives `resource`: clear its edge and add ownership.
pub fn hand_over(task: &Arc<TaskControlBlock>, resource: ResourceId) {
    let mut inner = task.inner_exclusive_access();
    assert_eq!(inner.blocked_on, Some(resource));
    inner.blocked_on = None;
    inner.held_resources.push(resource);
}

/// Record the edge `task -> resource` before it joins the wait queue.
pub fn set_waiting(task: &Arc<TaskControlBlock>, resource: ResourceId) {
    task.inner_exclusive_access().blocked_on = Some(resource);
}

/// Drop every edge and ownership of `task` — called when a thread exits.
pub fn forget(task: &Arc<TaskControlBlock>) {
    let mut inner = task.inner_exclusive_access();
    inner.held_resources.clear();
    inner.blocked_on = None;
}

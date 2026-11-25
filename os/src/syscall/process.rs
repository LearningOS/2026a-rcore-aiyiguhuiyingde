//! Process management syscalls
use alloc::sync::Arc;

use crate::{
    config::{PAGE_SIZE, TRAMPOLINE},
    loader::get_app_data_by_name,
    mm::{translated_refmut, translated_str, MapPermission, VirtAddr},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, TaskControlBlock,
    },
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!("kernel::pid[{}] sys_waitpid [{}]", current_task().unwrap().pid.0, pid);
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// get time with second and microsecond
///
/// The sixteen bytes of a `TimeVal` may straddle a page boundary, so they are
/// copied one byte at a time and every byte address is walked through the page
/// table on its own.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    let us = get_time_us();
    let word = core::mem::size_of::<usize>();
    let mut raw = [0u8; 2 * core::mem::size_of::<usize>()];
    raw[..word].copy_from_slice(&(us / 1_000_000).to_ne_bytes());
    raw[word..].copy_from_slice(&(us % 1_000_000).to_ne_bytes());
    let token = current_user_token();
    let base = ts as *mut u8;
    for (offset, byte) in raw.iter().enumerate() {
        *translated_refmut(token, unsafe { base.add(offset) }) = *byte;
    }
    0
}

/// Turn the `PROT_READ | PROT_WRITE | PROT_EXEC` bits of a mapping request into
/// page table flags for a page of user space.
///
/// Bits outside those three, a request of `0` and write-without-read are all
/// refused: SV39 does not allow a writable page that cannot be read.
fn perm_from_prot(port: usize) -> Option<MapPermission> {
    const READ: usize = 0b001;
    const WRITE: usize = 0b010;
    const EXEC: usize = 0b100;
    if port == 0 || port & !(READ | WRITE | EXEC) != 0 {
        return None;
    }
    if port & WRITE != 0 && port & READ == 0 {
        return None;
    }
    let mut perm = MapPermission::U;
    let flags = [
        (READ, MapPermission::R),
        (WRITE, MapPermission::W),
        (EXEC, MapPermission::X),
    ];
    for (bit, flag) in flags.iter() {
        if port & bit != 0 {
            perm |= *flag;
        }
    }
    Some(perm)
}

/// The last page number, exclusive, that a mapping of `len` bytes at a
/// page-aligned `start` really covers.
///
/// `len` is rounded up — `ch4_unmap` maps `len + 1` bytes and expects the
/// second page to show up — while an unaligned `start`, an empty request and a
/// span that reaches into the trampoline are refused.
fn mmap_span(start: usize, len: usize) -> Option<usize> {
    if len == 0 || start % PAGE_SIZE != 0 {
        return None;
    }
    let pages = len.checked_add(PAGE_SIZE - 1)? / PAGE_SIZE;
    let end = start.checked_add(pages.checked_mul(PAGE_SIZE)?)?;
    if end >= TRAMPOLINE {
        return None;
    }
    Some(end)
}

/// mmap: map anonymous pages of `[start, start + round_up(len))` into the
/// current address space and answer `0` on success.
///
/// Overlapping an existing mapping is refused rather than shadowed, because
/// [`crate::mm::MemorySet::insert_framed_area`] asserts on exactly that.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    let (perm, end) = match (perm_from_prot(port), mmap_span(start, len)) {
        (Some(perm), Some(end)) => (perm, end),
        _ => return -1,
    };
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    let (from_va, to_va) = (VirtAddr::from(start), VirtAddr::from(end));
    if !inner.memory_set.is_range_free(from_va, to_va) {
        return -1;
    }
    inner.memory_set.insert_framed_area(from_va, to_va, perm);
    0
}

/// munmap: drop the mapping of an exact, whole-page span of the current address
/// space.
///
/// Unlike [`sys_mmap`] the length is not rounded up: `ch4_unmap2` asks for one
/// byte more than it owns, and for a range starting mid-page, and expects `-1`
/// both times. What is removed must line up with the areas the process owns,
/// which is checked by [`crate::mm::MemorySet::unmap_range`].
pub fn sys_munmap(start: usize, len: usize) -> isize {
    if len == 0 || len % PAGE_SIZE != 0 || start % PAGE_SIZE != 0 {
        return -1;
    }
    let end = match start.checked_add(len) {
        Some(end) if end < TRAMPOLINE => end,
        _ => return -1,
    };
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if inner
        .memory_set
        .unmap_range(VirtAddr::from(start), VirtAddr::from(end))
    {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// spawn: run the program named by `path` as a brand new child process.
///
/// `fork` + `exec` reaches the same end state but pays for it twice: the whole
/// address space of the parent is copied first and thrown away a moment later
/// by the loader. Here the child's space is built straight from the elf image,
/// so only the parent/child link is taken from the caller — which is what lets
/// `wait` and `waitpid` reap it.
pub fn sys_spawn(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let elf_data = match get_app_data_by_name(path.as_str()) {
        Some(data) => data,
        None => return -1,
    };
    let parent = current_task().unwrap();
    let child = Arc::new(TaskControlBlock::new(elf_data));
    let child_pid = child.pid.0;
    child.inner_exclusive_access().parent = Some(Arc::downgrade(&parent));
    parent.inner_exclusive_access().children.push(Arc::clone(&child));
    add_task(child);
    child_pid as isize
}

/// Set the priority of the calling task and answer with the value accepted.
///
/// The scheduler divides [`crate::config::BIG_STRIDE`] by the priority to get
/// the charge per dispatch, so a priority of one already means the plain share
/// of the CPU and anything at or below it — including the negative values a
/// caller may hand us — is rejected with `-1` and changes nothing.
pub fn sys_set_priority(prio: isize) -> isize {
    if prio <= 1 {
        return -1;
    }
    current_task().unwrap().set_priority(prio as usize);
    prio
}

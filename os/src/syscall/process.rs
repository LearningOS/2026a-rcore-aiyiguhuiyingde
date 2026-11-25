//! Process management syscalls
//!
use alloc::sync::Arc;

use crate::{
    config::{PAGE_SIZE, TRAMPOLINE},
    fs::{open_file, OpenFlags},
    mm::{translated_refmut, translated_str, MapPermission, VirtAddr},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, TaskControlBlock,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
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
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
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

/// Get time with second and microsecond.
///
/// The `TimeVal` is filled by hand rather than assigned through one reference
/// because the sixteen bytes of the struct may straddle a page boundary: the
/// page table translates the first of them and the pair of frames behind the
/// two pages are no neighbours, so a single wide write would drop eight bytes
/// on the floor of the wrong frame. Each byte is walked on its own instead.
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

/// Turn the `PROT_READ | PROT_WRITE | PROT_EXEC` bits of a mapping request
/// into the page table flags of a user page.
///
/// Anything outside those three bits, a request of `0` and a write-only page
/// are all refused: SV39 has no way to make a page writable without also
/// letting it be read.
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

/// The first page number past a mapping of `len` bytes at `start`.
///
/// `len` rounds up to whole pages — `ch4_unmap` maps one byte more than a
/// page and expects the second page to be there — while a `start` that is not
/// page aligned, an empty request and a span that reaches the trampoline are
/// refused. Checked arithmetic, because a caller can hand us `usize::MAX`.
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

/// Map anonymous pages over `[start, start + round_up(len))` and answer `0`.
///
/// Overlapping a mapping that already exists is refused instead of shadowed,
/// which is what [`crate::mm::MemorySet::insert_framed_area`] would do.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    let (perm, end) = match (perm_from_prot(port), mmap_span(start, len)) {
        (Some(perm), Some(end)) => (perm, end),
        _ => return -1,
    };
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    let (from_va, to_va) = (VirtAddr::from(start), VirtAddr::from(end));
    if !inner.memory_set.span_is_free(from_va, to_va) {
        return -1;
    }
    inner.memory_set.insert_framed_area(from_va, to_va, perm);
    0
}

/// Drop the mapping of an exact span of whole pages of the address space.
///
/// The length is not rounded up here, unlike [`sys_mmap`]: `ch4_unmap2` asks
/// for one byte more than it owns, and for a range that starts mid page, and
/// expects `-1` both times. What is taken down also has to line up with the
/// areas the process owns, which
/// [`crate::mm::MemorySet::unmap_span`] is the one to decide.
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
        .unmap_span(VirtAddr::from(start), VirtAddr::from(end))
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

/// Spawn the program named by `path` as a new child of the caller.
///
/// `fork` and then `exec` arrives at the same end state and pays for it twice
/// over: the address space of the parent is copied page by page only to be
/// thrown away a moment later by the loader. Here the child is built straight
/// from the elf that easy-fs gives us, so the only thing taken from the caller
/// is the parent/child link — which is what lets `wait` and `waitpid` reap the
/// child the same way they reap a forked one.
pub fn sys_spawn(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let app_inode = match open_file(path.as_str(), OpenFlags::RDONLY) {
        Some(inode) => inode,
        None => return -1,
    };
    let elf_data = app_inode.read_all();
    let parent = current_task().unwrap();
    let child = Arc::new(TaskControlBlock::new(elf_data.as_slice()));
    let child_pid = child.pid.0;
    child.inner_exclusive_access().parent = Some(Arc::downgrade(&parent));
    parent.inner_exclusive_access().children.push(Arc::clone(&child));
    add_task(child);
    child_pid as isize
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(_prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    -1
}

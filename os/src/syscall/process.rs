//! Process management syscalls
use crate::config::{PAGE_SIZE, TRAMPOLINE};
use crate::mm::{read_user_byte, write_user_byte, MapPermission, VirtAddr, write_user_bytes};
use crate::task::{
    change_program_brk, count_task_syscall, current_user_token, exit_current_and_run_next,
    map_pages_of_current_task, suspend_current_and_run_next, unmap_pages_of_current_task,
};
use crate::timer::get_time_us;

/// `TraceRequest` of user_lib, a `#[repr(C)]` enum without explicit values: the
/// discriminants are the declaration order, so keep these three in sync with it.
const TRACE_READ: usize = 0;
const TRACE_WRITE: usize = 1;
const TRACE_SYSCALL: usize = 2;

/// Time stamp exchanged with user space. `user_lib::TimeVal` declares the same
/// two `usize` fields under `#[repr(C)]`, so this layout is what lands in the
/// caller's struct.
#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
///
/// The destination is a *user* address, so we go through the current task's page
/// table rather than dereferencing `ts`: the kernel satp does not map user pages.
/// `sec` and `usec` are written as one byte range that may straddle two pages,
/// which is what the "TimeVal split by two pages" HINT asks for. A pointer that
/// is unmapped or lacks R/W makes the copy fail and we answer `-1`, because a
/// timestamp call must never be able to kill the caller with a page fault.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    // Two usize fields, laid out by #[repr(C)] with no padding on RV64.
    let mut raw = [0u8; 2 * core::mem::size_of::<usize>()];
    raw[..core::mem::size_of::<usize>()].copy_from_slice(&(us / 1_000_000).to_ne_bytes());
    raw[core::mem::size_of::<usize>()..].copy_from_slice(&(us % 1_000_000).to_ne_bytes());
    if write_user_bytes(current_user_token(), ts as usize, &raw) {
        0
    } else {
        -1
    }
}

/// sys_trace: read a byte of this task's memory, overwrite a byte of it, or
/// report how often this task has invoked a given syscall id.
///
/// All three answers come from the current task only — ch4 has no way to name
/// another process — and the memory requests are resolved through its page
/// table, so an address the task does not own (kernel page, trampoline, a range
/// it just unmapped) is reported as an error instead of faulting the kernel.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!(
        "kernel: sys_trace request = {}, id = {:#x}",
        trace_request,
        id
    );
    match trace_request {
        // Read: the byte is zero-extended on purpose. user_lib maps any return
        // value other than -1 to Some(byte), so a byte >= 0x80 must not be
        // sign-extended into a negative isize and shown as None.
        TRACE_READ => match read_user_byte(current_user_token(), id) {
            Some(byte) => u32::from(byte) as isize,
            None => -1,
        },
        TRACE_WRITE => {
            if write_user_byte(current_user_token(), id, data as u8) {
                0
            } else {
                -1
            }
        }
        TRACE_SYSCALL => match count_task_syscall(id) {
            Some(count) => count as isize,
            None => -1,
        },
        _ => -1,
    }
}

/// Turn an mmap protection mask into PTE permissions.
///
/// Only R/W/X are meaningful, and `W` or `X` without `R` is rejected outright:
/// SV39 treats W=1,R=0 as reserved, so accepting it would install a PTE whose
/// real behaviour differs from what user asked for. `ch4_mmap2` relies on this
/// (`prot = 2` must fail).
fn prot_to_perm(prot: usize) -> Option<MapPermission> {
    const PROT_READ: usize = 1;
    const PROT_WRITE: usize = 2;
    const PROT_EXEC: usize = 4;
    if prot == 0 || prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return None;
    }
    if prot & PROT_READ == 0 && prot & (PROT_WRITE | PROT_EXEC) != 0 {
        return None;
    }
    let mut perm = MapPermission::U;
    if prot & PROT_READ != 0 {
        perm |= MapPermission::R;
    }
    if prot & PROT_WRITE != 0 {
        perm |= MapPermission::W;
    }
    if prot & PROT_EXEC != 0 {
        perm |= MapPermission::X;
    }
    Some(perm)
}

/// `[start, start + len)` rounded out to whole pages, or `None` when the request
/// is not page aligned at `start`, is empty, or wraps around.
///
/// `len` itself need not be a multiple of the page size: `ch4_unmap` maps
/// `len + 1` bytes and expects both pages to appear, while `ch4_mmap3` asks for
/// the same thing where the pages are still occupied and expects `-1`. Only the
/// free/occupied test can tell those two apart, so rounding up is required.
fn page_range(start: usize, len: usize) -> Option<(usize, usize)> {
    if len == 0 || start % PAGE_SIZE != 0 {
        return None;
    }
    let pages = len.saturating_add(PAGE_SIZE - 1) / PAGE_SIZE;
    let rounded = pages.checked_mul(PAGE_SIZE)?;
    start
        .checked_add(rounded)
        .filter(|&end| end <= TRAMPOLINE)
        .map(|end| (start, end))
}

/// Map anonymous zero-filled pages. Frames come from the frame allocator, which
/// clears each page, so a fresh mapping really reads back as zeros.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel: sys_mmap [{:#x}, {:#x}) prot = {:#x}", start, start + len, prot);
    let perm = match prot_to_perm(prot) {
        Some(perm) => perm,
        None => return -1,
    };
    let (from, to) = match page_range(start, len) {
        Some(range) => range,
        None => return -1,
    };
    map_pages_of_current_task(VirtAddr::from(from), VirtAddr::from(to), perm)
}

/// Drop the mapping of `[start, start + len)` and hand its frames back.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap [{:#x}, +{:#x})", start, len);
    let (from, to) = match page_range(start, len) {
        Some(range) => range,
        None => return -1,
    };
    unmap_pages_of_current_task(VirtAddr::from(from), VirtAddr::from(to))
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

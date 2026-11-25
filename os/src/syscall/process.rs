//! Process management syscalls
use crate::{
    loader::is_valid_app_addr,
    task::{
        current_task_id, exit_current_and_run_next, get_syscall_count, suspend_current_and_run_next,
    },
    timer::get_time_us,
};

/// Semantics of the `Syscall` trace request, as pinned by the official app:
///
/// ```text
/// assert_eq!(2, count_syscall(SYSCALL_TRACE));   // 1 earlier query + this one
/// assert_eq!(7, count_syscall(SYSCALL_TRACE));   // ...again including itself
/// ```
///
/// Trace requests count themselves ("注意这次 sys_trace 调用本身也计入"). No
/// special case is needed for that: the trap dispatcher bumps the histogram
/// before handing the request to `sys_trace`, so an in-flight query is already
/// in the numbers it reads back. Every other id is reported literally.
fn report_syscall_count(syscall_id: usize) -> Option<isize> {
    get_syscall_count(syscall_id)
}

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
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

/// The three request kinds accepted by `sys_trace`, matching `TraceRequest` in
/// user_lib (a plain `enum` without an explicit repr, so the discriminants are
/// declaration order: Read = 0, Write = 1, Syscall = 2).
const TRACE_REQ_READ: usize = 0;
const TRACE_REQ_WRITE: usize = 1;
const TRACE_REQ_SYSCALL: usize = 2;

/// sys_trace(trace_request, id, data) — a small introspection channel:
///
/// | trace_request | id    | data   | returns                                       |
/// |---------------|-------|--------|-----------------------------------------------|
/// | Read          | addr  | -      | byte at `addr` (unsigned), or -1 if invalid   |
/// | Write         | addr  | byte   | 0 on success, -1 if `addr` is not addressable |
/// | Syscall       | syscall_id | -  | how often we invoked that syscall, or -1      |
///
/// All three act on the *current* task only; there is no cross-task tracing in
/// ch3 (no process handles to name another task with), which also keeps the
/// address checks below meaningful.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!(
        "kernel: sys_trace(request = {}, id = {:#x}, data = {:#x})",
        trace_request,
        id,
        data
    );
    match trace_request {
        TRACE_REQ_READ => {
            // In ch3 apps run flat, without page tables, so "the current task's
            // address space" is exactly its fixed load window plus the stack
            // slot it was launched with. Checking an address against those two
            // regions is our stand-in for ch4's `translated_byte_buffer`: it
            // rejects kernel and foreign-task addresses instead of silently
            // dereferencing them.
            if !is_valid_app_addr(current_task_id(), id, 1) {
                return -1;
            }
            let byte = unsafe { (id as *const u8).read_volatile() };
            // Report through the kernel log so the read really is observable in
            // the serial output, not just returned in a register.
            info!(
                "[trace] task {} read byte {:#04x} from {:#x}",
                current_task_id(),
                byte,
                id
            );
            // Zero-extend, never sign-extend: user_lib turns any value other
            // than -1 into `Some(byte as u8)`, so a byte >= 0x80 must not be
            // widened into a negative isize and reported as `None`.
            byte as u32 as isize
        }
        TRACE_REQ_WRITE => {
            if !is_valid_app_addr(current_task_id(), id, 1) {
                return -1;
            }
            unsafe { (id as *mut u8).write_volatile(data as u8) };
            // volatile: the value must land in the app's memory before the trap
            // returns, since the app immediately re-reads it with
            // `read_volatile` and compares against the expected byte.
            0
        }
        TRACE_REQ_SYSCALL => match report_syscall_count(id) {
            Some(count) => {
                // `id` indexes our own histogram, so a query can never read
                // another task's counters.
                debug!(
                    "[trace] task {} syscall {} invoked {} time(s)",
                    current_task_id(),
                    id,
                    count
                );
                count
            }
            None => -1,
        },
        _ => -1,
    }
}

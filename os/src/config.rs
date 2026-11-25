//! Constants in the kernel

#[allow(unused)]

/// user app's stack size
pub const USER_STACK_SIZE: usize = 4096;
/// kernel stack size
pub const KERNEL_STACK_SIZE: usize = 4096 * 2;
/// kernel heap size
pub const KERNEL_HEAP_SIZE: usize = 0x20000;
/// the max number of apps
pub const MAX_APP_NUM: usize = 16;
/// base_addr(changed) of app
pub const APP_BASE_ADDRESS: usize = 0x80400000;
/// size limit of app
pub const APP_SIZE_LIMIT: usize = 0x20000;

/// clock frequency
pub const CLOCK_FREQ: usize = 12500000;
/// the physical memory end
pub const MEMORY_END: usize = 0x88000000;

/// Number of per-task syscall-counter slots.
///
/// Sized to cover the largest syscall id used by the test apps (`SYSCALL_TRACE`
/// = 410, `SYSCALL_THREAD_CREATE` = 460 in later chapters) with a little head
/// room, so that the table never needs a heap or a lookup indirection.
pub const SYSCALL_COUNT_SLOTS: usize = 512;

//! File and filesystem-related syscalls
use alloc::sync::Arc;

use crate::fs::{link_file, open_file, unlink_file, OpenFlags, Stat};
use crate::mm::{translated_byte_buffer, translated_refmut, translated_str, UserBuffer};
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// Report the metadata of an open file through its descriptor.
///
/// An fd is the whole argument here, so a descriptor that is out of range or
/// left over by `close` is refused, and so is stdin or stdout: they are not on
/// the disk and have nothing to report.
pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    trace!("kernel:pid[{}] sys_fstat", current_task().unwrap().pid.0);
    let file = {
        let task = current_task().unwrap();
        let inner = task.inner_exclusive_access();
        match inner.fd_table.get(fd).and_then(|slot| slot.as_ref()) {
            Some(file) => Arc::clone(file),
            None => return -1,
        }
    };
    let stat = match file.stat() {
        Some(stat) => stat,
        None => return -1,
    };
    // A stack variable of eighty bytes may well sit with part of itself in
    // one page and part in the next, and the two frames behind them need not
    // be neighbours, so every byte is walked through the page table by itself.
    let token = current_user_token();
    let src = &stat as *const Stat as *const u8;
    let dst = st as *mut u8;
    for i in 0..core::mem::size_of::<Stat>() {
        unsafe {
            *translated_refmut(token, dst.add(i)) = *src.add(i);
        }
    }
    0
}

/// Name an existing file a second time.
///
/// `-1` answers for a target that does not exist, a target that is a
/// directory, and a new name that is already taken.
pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_linkat", current_task().unwrap().pid.0);
    let token = current_user_token();
    let old_name = translated_str(token, old_name);
    let new_name = translated_str(token, new_name);
    // user_lib passes AT_FDCWD in front of every name, which is the root
    // directory this kernel has, so the names are used as they stand.
    if link_file(old_name.as_str(), new_name.as_str()) {
        0
    } else {
        -1
    }
}

/// Remove one name from the root directory.
///
/// A name that is not there is an error; a name that is there loses the entry
/// and one link of the inode behind it, and the file disappears with its last
/// name, which is what an open descriptor never has to care about.
pub fn sys_unlinkat(name: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_unlinkat", current_task().unwrap().pid.0);
    let name = translated_str(current_user_token(), name);
    if unlink_file(name.as_str()) {
        0
    } else {
        -1
    }
}

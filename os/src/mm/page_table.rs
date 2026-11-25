//! Implementation of [`PageTableEntry`] and [`PageTable`].

use super::{frame_alloc, FrameTracker, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
use crate::config::{PAGE_SIZE, PAGE_SIZE_BITS};
use alloc::vec;
use alloc::vec::Vec;
use bitflags::*;

bitflags! {
    /// page table entry flags
    pub struct PTEFlags: u8 {
        /// Valid
        const V = 1 << 0;
        /// Readable
        const R = 1 << 1;
        /// Writable
        const W = 1 << 2;
        /// eXecutable
        const X = 1 << 3;
        /// User
        const U = 1 << 4;
        /// Global
        const G = 1 << 5;
        /// Accessed
        const A = 1 << 6;
        /// Dirty
        const D = 1 << 7;
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
/// page table entry structure
pub struct PageTableEntry {
    /// bits of page table entry
    pub bits: usize,
}

impl PageTableEntry {
    /// Create a new page table entry
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        PageTableEntry {
            bits: ppn.0 << 10 | flags.bits as usize,
        }
    }
    /// Create an empty page table entry
    pub fn empty() -> Self {
        PageTableEntry { bits: 0 }
    }
    /// Get the physical page number from the page table entry
    pub fn ppn(&self) -> PhysPageNum {
        (self.bits >> 10 & ((1usize << 44) - 1)).into()
    }
    /// Get the flags from the page table entry
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits(self.bits as u8).unwrap()
    }
    /// The page pointered by page table entry is valid?
    pub fn is_valid(&self) -> bool {
        (self.flags() & PTEFlags::V) != PTEFlags::empty()
    }
    /// The page pointered by page table entry is readable?
    pub fn readable(&self) -> bool {
        (self.flags() & PTEFlags::R) != PTEFlags::empty()
    }
    /// The page pointered by page table entry is writable?
    pub fn writable(&self) -> bool {
        (self.flags() & PTEFlags::W) != PTEFlags::empty()
    }
    /// The page pointered by page table entry is executable?
    pub fn executable(&self) -> bool {
        (self.flags() & PTEFlags::X) != PTEFlags::empty()
    }
}

/// page table structure
pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}

/// Assume that it won't oom when creating/mapping.
impl PageTable {
    /// Create a new page table
    pub fn new() -> Self {
        let frame = frame_alloc().unwrap();
        PageTable {
            root_ppn: frame.ppn,
            frames: vec![frame],
        }
    }
    /// Temporarily used to get arguments from user space.
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from(satp & ((1usize << 44) - 1)),
            frames: Vec::new(),
        }
    }
    /// Find PageTableEntry by VirtPageNum, create a frame for a 4KB page table if not exist
    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                let frame = frame_alloc().unwrap();
                *pte = PageTableEntry::new(frame.ppn, PTEFlags::V);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        result
    }
    /// Find PageTableEntry by VirtPageNum
    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;
        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];
            if i == 2 {
                result = Some(pte);
                break;
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        result
    }
    /// set the map between virtual page number and physical page number
    #[allow(unused)]
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();
        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn);
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }
    /// remove the map between virtual page number and physical page number
    #[allow(unused)]
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);
        *pte = PageTableEntry::empty();
    }
    /// get the page table entry from the virtual page number
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }
    /// The leaf entry of `vpn` only counts as a mapping when it is valid: after
    /// `sys_munmap` the entry is zeroed but its parent tables survive, so a bare
    /// `translate(..).is_some()` would report freed pages as still mapped.
    pub fn is_mapped(&self, vpn: VirtPageNum) -> bool {
        matches!(self.translate(vpn), Some(pte) if pte.is_valid())
    }
    /// Walk `va` down to the physical frame holding it, rejecting an address
    /// whose walk stops early or whose leaf entry is not valid.
    fn fetch_frame(&self, va: VirtAddr) -> Option<(PhysPageNum, PTEFlags)> {
        let pte = self.translate(va.floor())?;
        if !pte.is_valid() {
            return None;
        }
        Some((pte.ppn(), pte.flags()))
    }
    /// get the token from the page table
    pub fn token(&self) -> usize {
        8usize << 60 | self.root_ppn.0
    }
}

/// Upper bound of the SV39 user half (`PAGE_SIZE_BITS + VPN_WIDTH_SV39` bits of
/// address): `[0, 2^38)` is what a U-mode program may own, everything above
/// belongs to the kernel and must never be reachable from a syscall argument.
const USER_VA_LIMIT: usize = 1 << (PAGE_SIZE_BITS + 27);

/// Is `[start, start + len)` a non-wrapping range inside the user half? Checking
/// this before touching the page table is what keeps a garbage pointer passed by
/// a user program from turning into a kernel-side load/store fault.
/// The explicit bound also rejects the two's-complement bit pattern of a
/// negative `isize`, which a test may hand us as a "bogus" address.
fn in_user_range(start: usize, len: usize) -> bool {
    if start >= USER_VA_LIMIT {
        return false;
    }
    match start.checked_add(len) {
        Some(end) => end <= USER_VA_LIMIT,
        None => false,
    }
}

/// Copy one byte out of the address space selected by `token`.
/// `None` means "not readable": unmapped, invalid, outside user space, or a page
/// whose R bit is clear — exactly the cases `sys_trace(Read)` must report as an
/// error instead of dereferencing.
pub fn read_user_byte(token: usize, va: usize) -> Option<u8> {
    if !in_user_range(va, 1) {
        return None;
    }
    let page_table = PageTable::from_token(token);
    let virt_va: VirtAddr = va.into();
    let (ppn, flags) = page_table.fetch_frame(virt_va)?;
    if (flags & PTEFlags::R) == PTEFlags::empty() {
        return None;
    }
    // The offset inside the frame has to be added back: ppn alone points at byte
    // 0 of the page, which is how an earlier attempt wrote the TimeVal fields to
    // the wrong address and left the caller's struct at zero.
    Some(ppn.get_bytes_array()[virt_va.page_offset()])
}

/// Store one byte into the address space selected by `token`, refusing a target
/// that is not both writable and valid. Returns whether the byte landed.
pub fn write_user_byte(token: usize, va: usize, byte: u8) -> bool {
    if !in_user_range(va, 1) {
        return false;
    }
    let page_table = PageTable::from_token(token);
    let virt_va: VirtAddr = va.into();
    let (ppn, flags) = match page_table.fetch_frame(virt_va) {
        Some(frame) => frame,
        None => return false,
    };
    if (flags & (PTEFlags::R | PTEFlags::W)) != (PTEFlags::R | PTEFlags::W) {
        return false;
    }
    ppn.get_bytes_array()[virt_va.page_offset()] = byte;
    true
}

/// Write `src` into the user address space, page chunk by page chunk so a buffer
/// straddling a page boundary (the HINT about a split `TimeVal`) still works.
/// Every page of the range must be valid, readable and writable; otherwise
/// nothing beyond the failing chunk is written and the call reports `false`.
pub fn write_user_bytes(token: usize, va: usize, src: &[u8]) -> bool {
    let len = src.len();
    if len == 0 || !in_user_range(va, len) {
        return false;
    }
    let page_table = PageTable::from_token(token);
    let mut done = 0;
    while done < len {
        let cursor = VirtAddr::from(va + done);
        let (ppn, flags) = match page_table.fetch_frame(cursor) {
            Some(ok) => ok,
            None => return false,
        };
        if (flags & (PTEFlags::R | PTEFlags::W)) != (PTEFlags::R | PTEFlags::W) {
            return false;
        }
        let page = ppn.get_bytes_array();
        let chunk = (PAGE_SIZE - cursor.page_offset()).min(len - done);
        let from = cursor.page_offset();
        page[from..from + chunk].copy_from_slice(&src[done..done + chunk]);
        done += chunk;
    }
    true
}

/// Translate&Copy a ptr[u8] array with LENGTH len to a mutable u8 Vec through page table
pub fn translated_byte_buffer(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut v = Vec::new();
    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();
        let ppn = page_table.translate(vpn).unwrap().ppn();
        vpn.step();
        let mut end_va: VirtAddr = vpn.into();
        end_va = end_va.min(VirtAddr::from(end));
        if end_va.page_offset() == 0 {
            v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..]);
        } else {
            v.push(&mut ppn.get_bytes_array()[start_va.page_offset()..end_va.page_offset()]);
        }
        start = end_va.into();
    }
    v
}

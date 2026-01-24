#![allow(dead_code)]
use crate::logger;
use crate::sys::memory::{alloc_pages, dealloc_pages, unmap_user_pages};
use crate::sys::proc::PROCESS_TABLE;
use crate::sys::proc::mem::{MmapKind, PAGE, align_up};
use twilight_common::syscall::types::EIO;

// minimal flag bits
#[allow(dead_code)]
pub const PROT_READ: usize = 1;
pub const PROT_WRITE: usize = 2;
#[allow(dead_code)]
pub const PROT_EXEC: usize = 4;

#[allow(dead_code)]
pub const MAP_SHARED: usize = 0x01;
pub const MAP_PRIVATE: usize = 0x02;
pub const MAP_FIXED: usize = 0x10;
pub const MAP_ANONYMOUS: usize = 0x20;

const EINVAL: i64 = -22;
const ENOMEM: i64 = -12;
const ENOSYS: i64 = -38;
const ESRCH: i64 = -3;
const EBADF: i64 = -9;

pub fn mmap(addr: u64, size: usize, prot: usize, flags: usize, fd: u64, offset: u64) -> i64 {
    #[allow(static_mut_refs)]
    let proc = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let process = match proc {
        Some(p) => p,
        None => return ESRCH,
    };

    if size == 0 {
        return EINVAL;
    }
    // if (flags & MAP_ANONYMOUS) == 0 {
    //     return ENOSYS;
    // } // file-backed not implemented yet
    // if (flags & MAP_PRIVATE) == 0 {
    //     return ENOSYS;
    // } // keep it simple for now
    // if (offset as usize) & (crate::sys::proc::mem::PAGE - 1) != 0 {
    //     return EINVAL;
    // } // must be page-aligned

    let len = align_up(size, PAGE);
    let writable = (prot & PROT_WRITE) != 0;
    let executable = (prot & PROT_EXEC) != 0;

    if (offset as usize) & (PAGE - 1) != 0 {
        return EINVAL;
    }

    let is_file_backed = (flags & MAP_ANONYMOUS) == 0 && (fd as i64) != -1;

    let va = if (flags & MAP_FIXED) != 0 {
        if addr == 0 || (addr as usize & (PAGE - 1)) != 0 {
            return EINVAL;
        }
        addr as usize
    } else {
        // ignore addr if 0; otherwise you can treat it as a hint later
        match process.proc_mm.reserve_mmap_range(len) {
            Some(v) => v,
            None => return ENOMEM,
        }
    };

    // never map page 0
    if va == 0 {
        return EINVAL;
    }

    if is_file_backed {
        // File-backed mapping:
        // - First, give the underlying node a chance to provide a real mapping (e.g. /dev/fb0).
        // - If it doesn't support mmap (ENOSYS), fall back to a generic "read file into pages"
        //   implementation for regular files (needed by dynamic loaders).
        let fd_i32 = fd as i32;
        if fd_i32 < 0 || fd_i32 < 3 {
            return EBADF;
        }
        let idx = (fd_i32 - 3) as usize;
        let Some(entry) = process.fd_table.get(idx).and_then(|slot| slot.as_ref()) else {
            return EBADF;
        };

        let file_ref = entry.file.clone();
        let file = file_ref.lock();
        let mut node = file.node.lock();
        if node.metadata.file_type == crate::sys::fs::vfs::FileType::Dir {
            return EINVAL;
        }

        match node.mmap(process, va, len, prot, flags, offset as usize) {
            Ok(mapped) => {
                process.proc_mm.track_mmap(mapped, len, MmapKind::Shared);
                return mapped as i64;
            }
            Err(-38) => {
                // ENOSYS: continue with generic mapping for regular files.
            }
            Err(code) => return code as i64,
        }

        // Map writable while populating (we don't have full mprotect/flag updates yet).
        if let Err(_) = alloc_pages(&mut process.mapper, va as u64, len, true, executable) {
            return ENOMEM;
        }

        let file_size = node.metadata.size;
        let mut remaining = len;
        let mut pos = 0usize;
        let mut file_off = offset as usize;

        // Copy file bytes into mapped memory; remaining bytes are already zeroed by alloc_pages.
        while remaining > 0 && file_off < file_size {
            let chunk = core::cmp::min(remaining, file_size - file_off);
            let dst = unsafe { core::slice::from_raw_parts_mut((va + pos) as *mut u8, chunk) };
            match node.read(file_off, dst) {
                Ok(0) => break,
                Ok(n) => {
                    pos += n;
                    file_off += n;
                    remaining = remaining.saturating_sub(n);
                }
                Err(_) => return -(EIO as i64),
            }
        }

        // Track as "shared" (close enough for now).
        process.proc_mm.track_mmap(va, len, MmapKind::Shared);
    } else {
        if let Err(_) = alloc_pages(&mut process.mapper, va as u64, len, writable, executable) {
            return ENOMEM;
        }
        process.proc_mm.track_mmap(va, len, MmapKind::Owned);
    }
    // logger!(
    //     "mmap: addr=0x{:x}, size={}, prot=0x{:x}, flags=0x{:x}, fd=0x{:x}, offset=0x{:x} => 0x{:x}",
    //     addr,
    //     size,
    //     prot,
    //     flags,
    //     fd,
    //     offset,
    //     va
    // );
    va as i64
}

pub fn mprotect(addr: u64, size: usize, prot: usize) -> i64 {
    if size == 0 {
        return EINVAL;
    }
    // logger!(
    //     "mprotect: addr=0x{:x}, size=0x{:x}, prot=0x{:x}",
    //     addr,
    //     size,
    //     prot
    // );
    0
}

pub fn brk(addr: usize) -> i64 {
    #[allow(static_mut_refs)]
    let proc = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let process = match proc {
        Some(p) => p,
        None => return -3, /* -ESRCH */
    };

    if addr == 0 {
        // logger!(
        //     "brk:- addr: {:#X} => {:#X}",
        //     addr,
        //     process.proc_mm.curr_brk()
        // );
        return process.proc_mm.curr_brk() as i64; // report current break
    }
    let res = match process.proc_mm.set_brk(&mut process.mapper, addr) {
        Ok(end) => end as i64,                       // success: return new break
        Err(_) => process.proc_mm.curr_brk() as i64, // failure: return current break
    };

    // logger!("brk:- addr: {:#X} => {:#X}", addr, res);
    res
}

pub fn munmap(addr: u64, size: usize) -> i64 {
    #[allow(static_mut_refs)]
    let Some(p) = (unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    }) else {
        return -1;
    };

    if size == 0 {
        return EINVAL;
    }

    if (addr as usize) & (PAGE - 1) != 0 {
        return EINVAL;
    }

    let len = align_up(size, PAGE);
    let base = addr as usize;
    let Some(kind) = p.proc_mm.remove_mmap(base, len) else {
        return EINVAL;
    };

    match kind {
        MmapKind::Owned => {
            if let Err(()) = dealloc_pages(&mut p.mapper, addr, len) {
                return -1;
            }
        }
        MmapKind::Shared => {
            if let Err(()) = unmap_user_pages(&mut p.mapper, addr, len) {
                return -1;
            }
        }
    }

    0
}

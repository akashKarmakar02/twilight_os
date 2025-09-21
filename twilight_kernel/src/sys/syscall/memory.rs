use crate::logger;
use crate::sys::memory::alloc_pages;
use crate::sys::proc::mem::align_up;
use crate::sys::proc::PROCESS_TABLE;

// minimal flag bits
const PROT_READ:  usize = 1;
const PROT_WRITE: usize = 2;
const PROT_EXEC:  usize = 4;

const MAP_SHARED:    usize = 0x01;
const MAP_PRIVATE:   usize = 0x02;
const MAP_FIXED:     usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;

const EINVAL: i64 = -22;
const ENOMEM: i64 = -12;
const ENOSYS: i64 = -38;
const ESRCH:  i64 = -3;

pub(crate) fn mmap(addr: u64, size: usize, prot: usize, flags: usize, fd: u64, offset: u64) -> i64 {
    #[allow(static_mut_refs)]
    let proc = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(crate::sys::proc::id()) };
    let process = match proc { Some(p) => p, None => return ESRCH };

    if size == 0 { return EINVAL; }
    if (flags & MAP_ANONYMOUS) == 0 { return ENOSYS; }         // file-backed not implemented yet
    if (flags & MAP_PRIVATE) == 0   { return ENOSYS; }          // keep it simple for now
    if (offset as usize) & (crate::sys::proc::mem::PAGE - 1) != 0 { return EINVAL; }   // must be page-aligned

    let len = align_up(size, crate::sys::proc::mem::PAGE);
    let writable = (prot & PROT_WRITE) != 0;

    let va = if (flags & MAP_FIXED) != 0 {
        if addr == 0 || (addr as usize & (crate::sys::proc::mem::PAGE - 1)) != 0 { return EINVAL; }
        addr as usize
    } else {
        // ignore addr if 0; otherwise you can treat it as a hint later
        match process.proc_mm.reserve_mmap_range(len) {
            Some(v) => v,
            None => return ENOMEM,
        }
    };

    // never map page 0
    if va == 0 { return EINVAL; }

    if let Err(_) = alloc_pages(&mut process.mapper, va as u64, len, true, writable) {
        return ENOMEM;
    }
    logger!("mmap: addr=0x{:x}, size=0x{:x}, prot=0x{:x}, flags=0x{:x}, fd=0x{:x}, offset=0x{:x} => 0x{:x}", addr, size, prot, flags, fd, offset, va);
    va as i64
}

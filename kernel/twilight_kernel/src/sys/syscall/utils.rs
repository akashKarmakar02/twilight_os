use crate::sys::proc::PROCESS_TABLE;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const USER_MAX: usize = 0x0000_7fff_ffff_ffff; // canonical userspace (48-bit, upper cleared)

#[repr(transparent)]
pub struct UserPtr<T>(pub *const T);
unsafe impl<T> Send for UserPtr<T> {}
unsafe impl<T> Sync for UserPtr<T> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCopyError {
    Fault,   // invalid mapping / page fault
    TooLong, // exceeded max length without NUL
    Utf8,    // not valid UTF-8 (optional, if you enforce)
}

/// Implement this using your page-table walker or copyin routine.
/// It must validate that `addr` is a canonical, user-mapped address and handle faults.
fn read_user_byte(addr: *const u8) -> Result<u8, UserCopyError> {
    // ---- stub / hook point ----
    // Option A (if you have a safe copyin that returns Result):
    //     copyin(addr, &mut byte).map(|_| byte).map_err(|_| UserCopyError::Fault)
    //
    // Option B (if user pages are directly accessible but may fault):
    //     unsafe { core::ptr::read_volatile(addr) }  <-- wrap in a fault catcher
    //
    // Option C: translate_user_va(addr) -> *const u8 in kernel mapping, then read.
    //
    // For now, assume you have:
    unsafe {
        if !is_user_accessible(addr as usize) {
            return Err(UserCopyError::Fault);
        }
        Ok(core::ptr::read_volatile(addr))
    }
}

/// Copy a NUL-terminated string from user space with a max cap.
pub fn copy_cstr_from_user(uptr: UserPtr<u8>, max: usize) -> Result<String, UserCopyError> {
    let mut out: Vec<u8> = Vec::new();

    for i in 0..max {
        // SAFETY: arithmetic on raw pointer, bounds enforced by `max`
        let p = unsafe { uptr.0.add(i) };
        let b = read_user_byte(p)?;
        if b == 0 {
            // Finished; optionally validate UTF-8 for POSIX paths (not required)
            return String::from_utf8(out).map_err(|_| UserCopyError::Utf8);
        }
        out.push(b);
    }

    Err(UserCopyError::TooLong)
}

// You likely already have something like this; placeholder here:
unsafe fn is_user_accessible(_va: usize) -> bool {
    // check canonical addr, U/S bit in PTEs, present, etc.
    true
}

#[inline(always)]
fn is_canonical(addr: usize) -> bool {
    let top = (addr >> 47) & 0x1ffff;
    top == 0 || top == 0x1ffff
}

#[inline(always)]
fn in_user_range(addr: usize, len: usize) -> Result<(), UserCopyError> {
    if addr == 0 || !is_canonical(addr) {
        return Err(UserCopyError::Fault);
    }
    let end = addr.checked_add(len).ok_or(UserCopyError::TooLong)?;
    if end - 1 > USER_MAX {
        return Err(UserCopyError::Fault);
    }
    Ok(())
}

/// Copy one `T` from a possibly-unaligned user pointer into kernel-owned
/// storage. The pointer is not retained after the call returns. Returns
/// `Err(Fault)` for a null, non-canonical, or out-of-range user address.
///
/// Used by time syscalls (`nanosleep`, `clock_nanosleep`) to read the packed
/// `Timespec` request into aligned kernel memory before any validation or
/// blocking, so no userspace reference survives across a context switch.
///
/// SAFETY: this is the fault-aware equivalent of `core::ptr::read_unaligned`.
/// The caller must ensure `src` is a userspace pointer (it is validated by
/// `in_user_range` before any deref). A page fault on a COW/zfod page is
/// resolved by the page-fault handler; a genuinely unmapped address is rejected
/// by the range/canonical check.
pub fn copy_from_user<T>(src: *const T) -> Result<T, UserCopyError>
where
    T: Copy,
{
    if src.is_null() {
        return Err(UserCopyError::Fault);
    }
    in_user_range(src as usize, size_of::<T>())?;
    // SAFETY: `in_user_range` verified the address is canonical and within the
    // user range. `read_volatile` avoids creating a reference into userspace
    // (the source may be unaligned — `Timespec` is `#[repr(C, packed)]`).
    unsafe { Ok(core::ptr::read_volatile(src)) }
}

/// Copy one `T` to a possibly-unaligned user pointer. Returns `Err(Fault)` for
/// a null, non-canonical, or out-of-range user address.
///
/// Used by time syscalls to write `clock_gettime` output and the `rem`
/// remainder on an interrupted relative `nanosleep`. The write is best-effort
/// per POSIX (Linux writes `rem` even when returning `-EINTR`); a fault here is
/// reported so the caller can decide whether to surface `-EFAULT`.
pub fn copy_to_user<T>(dst: *mut T, val: T) -> Result<(), UserCopyError>
where
    T: Copy,
{
    if dst.is_null() {
        return Err(UserCopyError::Fault);
    }
    in_user_range(dst as usize, size_of::<T>())?;
    // SAFETY: `in_user_range` verified the address. `write_volatile` avoids
    // creating a reference into userspace and tolerates an unaligned `dst`.
    unsafe {
        core::ptr::write_volatile(dst, val);
    }
    Ok(())
}

pub fn format_path(path: String) -> String {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap_unchecked()
            .get_process(crate::sys::proc::id())
            .unwrap_unchecked()
    };

    let can_path = if path.starts_with("/") {
        path
    } else {
        format!("{}/{}", process.pwd, path)
    };
    can_path
}

pub fn copy_user_ptr_array(
    base: UserPtr<usize>,
    max_ptrs: usize,
    max_str: usize,
) -> Result<Vec<String>, UserCopyError> {
    let mut out = Vec::new();
    for i in 0..max_ptrs {
        let ptr_addr = (base.0 as usize)
            .checked_add(i * size_of::<usize>())
            .ok_or(UserCopyError::TooLong)?;
        in_user_range(ptr_addr, size_of::<usize>())?;

        let uptr = unsafe { core::ptr::read_unaligned(ptr_addr as *const usize) };
        if uptr == 0 {
            break;
        } // NULL terminator
        let s = copy_cstr_from_user(UserPtr(uptr as *const u8), max_str)?;
        out.push(s);
    }
    Ok(out)
}

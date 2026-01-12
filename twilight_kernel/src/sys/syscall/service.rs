use crate::arch::x86_64::io::{IA32_FS_BASE, IA32_GS_BASE, rdmsr, wrmsr};
use crate::driver::disk::dummy_blockdev;
use crate::driver::timer::pit::uptime;
use crate::sys::console::{self, DIR, get_tty};
use crate::sys::fs::vfs::{FileType, VFS, VfsNodeOps};
use crate::sys::proc::{FdEntry, OpenFile, PROCESS_TABLE, Process, USER_STACK_SIZE};
use crate::sys::syscall::utils::{UserPtr, copy_cstr_from_user, copy_user_ptr_array, format_path};
use crate::task::executor::halt;
use crate::{logger, print, sys};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::arch::asm;
use spin::mutex::Mutex;
use twilight_common::syscall::types::*;

fn join_paths(base: &str, rel: &str) -> String {
    if rel.is_empty() || rel == "." {
        return base.to_string();
    }
    if rel.starts_with('/') {
        return rel.to_string();
    }
    if base == "/" {
        format!("/{}", rel.trim_start_matches('/'))
    } else {
        format!("{}/{}", base.trim_end_matches('/'), rel)
    }
}

#[inline(always)]
fn parent_path(path: &str) -> &str {
    // Remove trailing slash (except root)
    let path = if path != "/" && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    };

    // Find the last '/'
    match path.rfind('/') {
        Some(0) => "/", // parent of "/foo" is "/"
        Some(idx) => &path[..idx],
        None => ".", // no slash → current directory
    }
}

fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            out.pop();
        } else {
            out.push(seg);
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

const FD_CLOEXEC: i32 = 0x1;
const STATUS_FLAG_MUTABLE: i32 = O_APPEND | O_NONBLOCK;

fn status_flags_from_open(flags: i32) -> i32 {
    let mut status = flags & O_ACCMODE;
    status |= flags & (O_APPEND | O_NONBLOCK | O_DIRECTORY | O_PATH);
    status
}

fn fd_slot(process: &Process, fd: i32) -> Result<&FdEntry, i32> {
    if fd < 3 {
        return Err(-EBADF);
    }
    let idx = (fd - 3) as usize;
    match process.fd_table.get(idx) {
        Some(Some(entry)) => Ok(entry),
        _ => Err(-EBADF),
    }
}

fn fd_slot_mut(process: &mut Process, fd: i32) -> Result<&mut FdEntry, i32> {
    if fd < 3 {
        return Err(-EBADF);
    }
    let idx = (fd - 3) as usize;
    match process.fd_table.get_mut(idx) {
        Some(Some(entry)) => Ok(entry),
        _ => Err(-EBADF),
    }
}

fn clone_open_file(process: &Process, fd: i32) -> Result<Arc<Mutex<OpenFile>>, i32> {
    Ok(fd_slot(process, fd)?.file.clone())
}

fn install_fd_entry(process: &mut Process, entry: FdEntry, min_fd: i32) -> Result<i32, i32> {
    if min_fd < 0 {
        return Err(EINVAL);
    }
    let start_idx = min_fd.saturating_sub(3).max(0) as usize;
    for idx in start_idx..process.fd_table.len() {
        if process.fd_table[idx].is_none() {
            process.fd_table[idx] = Some(entry);
            return Ok((idx + 3) as i32);
        }
    }
    while process.fd_table.len() < start_idx {
        process.fd_table.push(None);
    }
    process.fd_table.push(Some(entry));
    Ok((process.fd_table.len() - 1 + 3) as i32)
}

fn set_stdio_status(process: &mut Process, fd: i32, value: i32) -> Result<(), i32> {
    if (0..=2).contains(&fd) {
        process.stdio_flags[fd as usize] = value;
        Ok(())
    } else {
        Err(-EBADF)
    }
}

fn get_stdio_status(process: &Process, fd: i32) -> Result<i32, i32> {
    if (0..=2).contains(&fd) {
        Ok(process.stdio_flags[fd as usize])
    } else {
        Err(-EBADF)
    }
}

fn set_stdio_fd_flags(process: &mut Process, fd: i32, value: i32) -> Result<(), i32> {
    if (0..=2).contains(&fd) {
        process.stdio_fd_flags[fd as usize] = value;
        Ok(())
    } else {
        Err(-EBADF)
    }
}

fn get_stdio_fd_flags(process: &Process, fd: i32) -> Result<i32, i32> {
    if (0..=2).contains(&fd) {
        Ok(process.stdio_fd_flags[fd as usize])
    } else {
        Err(-EBADF)
    }
}

fn base_for_dirfd(process: &mut Process, dirfd: i32) -> Result<String, i32> {
    if dirfd == AT_FDCWD {
        return Ok(process.pwd.clone());
    }
    if dirfd < 3 {
        return Err(-EBADF);
    }
    let entry = fd_slot(process, dirfd)?;
    let file = entry.file.lock();
    if file.node.lock().metadata.file_type != FileType::Dir {
        return Err(-ENOTDIR);
    }

    Ok(file.path.clone())
}
fn split_parent_name(path: &str) -> (&str, &str) {
    if let Some(p) = path.rfind('/') {
        if p == 0 {
            ("/", &path[1..])
        } else {
            (&path[..p], &path[p + 1..])
        }
    } else {
        (".", path)
    }
}

pub fn write(arg1: i32, arg2: usize, arg3: usize) -> i64 {
    let file_descriptor = arg1;
    let buf = arg2 as *const u8;
    let len = arg3;
    let buf = unsafe { core::slice::from_raw_parts(buf, len) };

    let res = match file_descriptor {
        1 => {
            if console::pipeline_write(buf) {
                len as i64
            } else {
                print!("{}", String::from_utf8_lossy(buf));
                len as i64
            }
        }
        2 => {
            print!("{}", String::from_utf8_lossy(buf));

            len as i64
        }
        n => {
            #[allow(static_mut_refs)]
            let process = unsafe {
                PROCESS_TABLE
                    .get_mut()
                    .unwrap()
                    .get_process(crate::sys::proc::id())
                    .unwrap()
            };

            match clone_open_file(process, n) {
                Ok(file_ref) => {
                    let mut file = file_ref.lock();
                    let accmode = file.status_flags & O_ACCMODE;
                    if accmode == O_RDONLY {
                        return -(EBADF as i64);
                    }
                    let append = (file.status_flags & O_APPEND) != 0;
                    let start = if append {
                        file.node.lock().metadata.size
                    } else {
                        file.seek
                    };
                    let end = start.saturating_add(len);

                    let result = {
                        let mut node = file.node.lock();
                        let r = node.write(start, buf);
                        if r.is_ok() && end > node.metadata.size {
                            node.metadata.size = end;
                        }
                        r
                    };

                    if result.is_ok() {
                        file.seek = end;
                        len as i64
                    } else {
                        -1
                    }
                }
                Err(code) => code as i64,
            }
        }
    };

    res
}

pub fn close(fd: i32) -> i64 {
    if fd < 0 {
        return -(EBADF as i64);
    }
    if fd <= 2 {
        return 0;
    }

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    let idx = (fd - 3) as usize;
    if let Some(slot) = process.fd_table.get_mut(idx) {
        if slot.take().is_some() {
            return 0;
        }
    }

    -(EBADF as i64)
}

pub fn read(fd: usize, buf: &mut [u8]) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    if fd <= 2 {
        if fd == 0 {
            if let Some(bytes) = console::pipeline_read(buf) {
                return bytes as i64;
            }

            let flags = process.stdio_flags[0];
            let tty = get_tty();
            let mut dev = dummy_blockdev();
            if (flags & O_NONBLOCK) != 0 {
                match tty.poll(&mut dev) {
                    Ok(true) => {}
                    Ok(false) => return -(EAGAIN as i64),
                    Err(_) => return -(EIO as i64),
                }
            }
            if let Ok(v) = tty.read(&mut dev, 0, buf) {
                return v as i64;
            }
        }
        return 0;
    }

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let accmode = file.status_flags & O_ACCMODE;
    if accmode == O_WRONLY {
        return -(EBADF as i64);
    }
    let mut vfs_node = file.node.lock();
    match vfs_node.metadata.file_type {
        FileType::Dir => -(EISDIR as i64),
        FileType::CharDevice => {
            if let Ok(content) = vfs_node.read(buf.len(), buf) {
                let copy_len = content.min(buf.len());
                copy_len as i64
            } else {
                -1
            }
        }
        _ => {
            let seek = file.seek;
            if let Ok(copy_len) = vfs_node.read(seek, buf) {
                drop(vfs_node); // Release the immutable borrow before modifying file
                file.seek += copy_len;
                copy_len as i64
            } else {
                -1
            }
        }
    }
}

pub fn open(path: &str, flags: i32, mode: u32) -> i64 {
    openat(AT_FDCWD, path, flags, mode)
}

pub fn openat(dirfd: i32, path: &str, flags: i32, mode: u32) -> i64 {
    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    // Resolve full path
    let full_path = if path.starts_with('/') {
        normalize_path(path)
    } else {
        match base_for_dirfd(process, dirfd) {
            Ok(base) => normalize_path(&join_paths(&base, path)),
            Err(e) => return e as i64,
        }
    };

    // Try open existing
    let mut existed = true;
    #[allow(static_mut_refs)]
    let node = unsafe { VFS.get_mut().open(&full_path) };
    let node = match (node, (flags & O_CREAT) != 0) {
        (Ok(n), _) => n,
        (Err(_), true) => {
            // create new file with mode
            let (parent, name) = split_parent_name(&full_path);
            // parent must exist and be a dir
            #[allow(static_mut_refs)]
            if let Ok(meta) = unsafe { VFS.get_mut().metadata(parent) } {
                if meta.file_type != FileType::Dir {
                    return -(ENOTDIR as i64);
                }
            } else {
                return -(ENOENT as i64);
            }

            #[allow(static_mut_refs)]
            if unsafe { VFS.get_mut().touch(parent, name, mode) }.is_err() {
                return -(EIO as i64);
            }
            existed = false;
            #[allow(static_mut_refs)]
            // reopen
            match unsafe { VFS.get_mut().open(&full_path) } {
                Ok(n2) => n2,
                Err(_) => return -(EIO as i64),
            }
        }
        (Err(_), false) => return -(ENOENT as i64),
    };

    // Enforce O_EXCL if it existed
    if existed && (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
        return -(EEXIST as i64);
    }

    // O_DIRECTORY: must be a directory
    if (flags & O_DIRECTORY) != 0 && node.metadata.file_type != FileType::Dir {
        return -(ENOTDIR as i64);
    }

    // Cannot open directory for write unless you implement it
    let accmode = flags & O_ACCMODE;
    if node.metadata.file_type == FileType::Dir && (accmode == O_WRONLY || accmode == O_RDWR) {
        return -(EISDIR as i64);
    }

    // O_TRUNC (only for regular files)
    if (flags & O_TRUNC) != 0 && node.metadata.file_type == FileType::File {
        // You don't have truncate: emulate by writing empty content
        // #[allow(static_mut_refs)]
        // if unsafe { VFS.get_mut().write(&full_path, &[]) }.is_err() {
        //     return -(EOPNOTSUPP as i64);
        // }
    }

    let mut initial_seek: usize = 0;
    if node.metadata.file_type == FileType::File {
        let file_len = node.metadata.size;
        if (flags & O_APPEND) != 0 {
            initial_seek = file_len;
        }
    }

    // Install FD
    let open_file = OpenFile {
        node: Arc::new(Mutex::new(node)),
        seek: initial_seek,
        path: full_path,
        status_flags: status_flags_from_open(flags),
    };
    let entry = FdEntry {
        file: Arc::new(Mutex::new(open_file)),
        fd_flags: if (flags & O_CLOEXEC) != 0 {
            FD_CLOEXEC
        } else {
            0
        },
    };
    match install_fd_entry(process, entry, 3) {
        Ok(fd) => fd as i64,
        Err(code) => -(code as i64),
    }
}

pub fn execev(arg1: usize, arg2: usize, _arg3: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(arg1 as *const u8), 4096) else {
        return -1;
    };

    #[allow(static_mut_refs)]
    let Ok(mut elf_node) = (unsafe { VFS.read().open(path.as_str().trim()) }) else {
        return -2;
    };

    let elf_size = elf_node.metadata.size;
    let mut elf_buf = vec![0u8; elf_size];

    let Ok(_) = elf_node.read(0, &mut elf_buf) else {
        return -2;
    };

    let argv = match copy_user_ptr_array(UserPtr(arg2 as *const usize), 128, 4096) {
        Ok(v) => v,
        Err(_) => return -1, // EFAULT
    };

    let argv = argv.iter().map(|p| p.as_str()).collect::<Vec<&str>>();

    #[allow(static_mut_refs)]
    let process_table = unsafe { PROCESS_TABLE.get_mut().unwrap() };

    let pwd = process_table
        .get_process(crate::sys::proc::id())
        .unwrap()
        .pwd
        .clone();

    if let Ok(p) = Process::new(
        elf_buf,
        pwd.as_str(),
        argv.as_slice(),
        crate::sys::proc::id(),
    ) {
        unsafe { asm!("swapgs") };
        process_table.run(p);
    } else {
        return -1;
    }

    0
}

pub fn exit() -> i64 {
    unsafe { asm!("swapgs") };

    crate::sys::proc::exit();

    unreachable!()
}

pub fn uname(ptr: usize) -> i64 {
    let uname_ptr = ptr as *mut UtsName;

    fn fill(buf: &mut [u8; 65], s: &str) {
        buf.fill(0);
        let bytes = s.as_bytes();
        let n = core::cmp::min(bytes.len(), 64); // leave room for NUL
        buf[..n].copy_from_slice(&bytes[..n]);
        buf[n] = 0;
    }

    if !uname_ptr.is_null() {
        unsafe {
            let uname_s = &mut *uname_ptr;

            fill(&mut uname_s.sysname, "TwilightOS");
            fill(&mut uname_s.nodename, "twilight");
            fill(&mut uname_s.release, "0.1.0-testing-build.x86_64");
            fill(&mut uname_s.version, "#1 NON-SMP 26-10-2025");
            fill(&mut uname_s.machine, "x86_64");
            fill(&mut uname_s.domainname, "-");
        }
    }

    0
}

pub fn arch_prctl(code: u64, addr: u64) -> i64 {
    logger!("arch_prctl: code=0x{:x}, arg=0x{:x}", code, addr);
    match code {
        ARCH_SET_FS => {
            wrmsr(IA32_FS_BASE, addr);
            0
        }
        ARCH_GET_FS => rdmsr(IA32_FS_BASE) as i64,
        ARCH_SET_GS => {
            wrmsr(IA32_GS_BASE, addr);
            0
        }
        ARCH_GET_GS => rdmsr(IA32_GS_BASE) as i64,
        _ => EINVAL as i64,
    }
}

pub fn writev(fd: i32, iov_ptr: u64, iovcnt: i32) -> i64 {
    if iovcnt < 0 {
        return -1;
    }
    let n = iovcnt as usize;

    // SAFETY: trusting user pointers here; in production, copy to kernel buffer
    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, n) };

    let mut total: i64 = 0;
    for iv in iov {
        // Skip empty segments
        if iv.iov_len == 0 {
            continue;
        }

        // Write this segment
        let r = write(fd, iv.iov_base as usize, iv.iov_len);
        if r < 0 {
            return r;
        }
        total = total.saturating_add(r);

        // Stop on partial write (short write semantics)
        if (r as usize) < iv.iov_len {
            break;
        }
    }
    total
}

pub fn fcntl(fd: i32, cmd: i32, arg: u64) -> i64 {
    const F_DUPFD: i32 = 0;
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;

    if fd < 0 {
        return -(EBADF as i64);
    }

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    match cmd {
        F_GETFD => {
            if fd <= 2 {
                return match get_stdio_fd_flags(process, fd) {
                    Ok(flags) => flags as i64,
                    Err(code) => code as i64,
                };
            }
            match fd_slot(process, fd) {
                Ok(entry) => entry.fd_flags as i64,
                Err(code) => code as i64,
            }
        }
        F_SETFD => {
            let new_flags = (arg as i32) & FD_CLOEXEC;
            if fd <= 2 {
                return match set_stdio_fd_flags(process, fd, new_flags) {
                    Ok(()) => 0,
                    Err(code) => code as i64,
                };
            }
            match fd_slot_mut(process, fd) {
                Ok(entry) => {
                    entry.fd_flags = (entry.fd_flags & !FD_CLOEXEC) | new_flags;
                    0
                }
                Err(code) => code as i64,
            }
        }
        F_GETFL => {
            if fd <= 2 {
                return match get_stdio_status(process, fd) {
                    Ok(flags) => flags as i64,
                    Err(code) => code as i64,
                };
            }
            match clone_open_file(process, fd) {
                Ok(file_ref) => file_ref.lock().status_flags as i64,
                Err(code) => code as i64,
            }
        }
        F_SETFL => {
            let new_bits = (arg as i32) & STATUS_FLAG_MUTABLE;
            if fd <= 2 {
                let current = process.stdio_flags[fd as usize];
                let preserved = current & !STATUS_FLAG_MUTABLE;
                let new_value = preserved | new_bits;
                return match set_stdio_status(process, fd, new_value) {
                    Ok(()) => 0,
                    Err(code) => code as i64,
                };
            }
            match clone_open_file(process, fd) {
                Ok(file_ref) => {
                    let mut file = file_ref.lock();
                    let preserved = file.status_flags & !STATUS_FLAG_MUTABLE;
                    file.status_flags = preserved | new_bits;
                    0
                }
                Err(code) => code as i64,
            }
        }
        F_DUPFD => {
            if fd <= 2 {
                return -(EBADF as i64);
            }
            if arg > i32::MAX as u64 {
                return -(EINVAL as i64);
            }
            let min_fd = (arg as i32).max(3);
            let src_entry = match fd_slot(process, fd) {
                Ok(entry) => entry,
                Err(code) => return code as i64,
            };
            let new_entry = FdEntry {
                file: src_entry.file.clone(),
                fd_flags: src_entry.fd_flags & !FD_CLOEXEC,
            };
            match install_fd_entry(process, new_entry, min_fd) {
                Ok(new_fd) => new_fd as i64,
                Err(code) => -(code as i64),
            }
        }
        _ => -(EINVAL as i64),
    }
}

pub fn pr_limit64(
    pid: i32,
    resource: u32,
    _new_limit_ptr: Option<&Rlimit64>,
    old_limit_ptr: Option<&mut Rlimit64>,
) -> i64 {
    if pid != 0 {
        return -ESRCH as i64;
    }
    if resource != RLIMIT_STACK {
        return -EINVAL as i64;
    }

    if let Some(old_limit_ptr) = old_limit_ptr {
        old_limit_ptr.rlim_max = USER_STACK_SIZE as u64;
        old_limit_ptr.rlim_cur = USER_STACK_SIZE as u64;
    }

    0
}

struct DirentItem {
    ino: u64,
    dtype: u8,
    name: String,
    reclen: u16, // computed record length including name+NUL+padding
    next_cookie: i64,
}
#[inline(always)]
fn dt_from_filetype(ft: FileType) -> u8 {
    // DT_* values (Linux): UNKNOWN=0,FIFO=1,CHR=2,DIR=4,BLK=6,REG=8,LNK=10,SOCK=12, WHT=14
    match ft {
        FileType::Dir => 4, // DT_DIR
        FileType::File => 8,
        FileType::CharDevice => 2,
        FileType::BlockDevice => 6, // DT_REG
    }
}
#[inline(always)]
fn dirent64_reclen(name_len: usize) -> u16 {
    // the header is 19 bytes (packed), then name + NUL, then 8B align
    let base = size_of::<Dirent64Hdr>(); // 19
    let need = base + name_len + 1;
    let aligned = (need + 7) & !7;
    aligned as u16
}

pub fn getdent64(fd: i32, user_buf: *mut u8, buf_len: usize) -> i64 {
    if user_buf.is_null() {
        return -(EFAULT as i64);
    }
    if buf_len < (size_of::<Dirent64Hdr>() + 2) {
        // practically can't hold any useful name
        return -(EINVAL as i64);
    }

    // Get process and FD
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    if file.node.lock().metadata.file_type != FileType::Dir {
        return -(ENOTDIR as i64);
    }

    // Read directory entries from VFS (adjust API if yours differs)
    #[allow(static_mut_refs)]
    let entries = match unsafe { VFS.get_mut().ls(&file.path) } {
        Ok(v) => v, // Vec<DirEntry { name:String, inode:u64, file_type:FileType }>
        Err(_) => return -(EIO as i64),
    };

    // Current position (use as entry index 'cookie')
    let mut idx = file.seek;
    if idx >= entries.len() {
        return 0; // EOF
    }

    // 1) Build a struct list with sizes precomputed
    let mut items: Vec<DirentItem> = Vec::new(); // or Vec if you prefer
    let mut total_needed = 0usize;

    for (i, e) in entries.iter().enumerate().skip(idx) {
        let dtype = dt_from_filetype(e.file_type);
        let reclen = dirent64_reclen(e.name.len());
        let reclen_usize = reclen as usize;

        if total_needed + reclen_usize > buf_len {
            break; // stop when buffer would overflow
        }

        items.push(DirentItem {
            ino: e.ino as u64,
            dtype,
            name: e.name.clone(),
            reclen,
            next_cookie: (i as i64) + 1, // “position cookie” to next entry
        }); // ignore overflow if you swap heapless for Vec

        total_needed += reclen_usize;
    }

    if items.is_empty() {
        // Buffer too small to fit the next entry -> return 0 only at EOF,
        // otherwise userspace will retry with a bigger buffer or next loop.
        if idx >= entries.len() {
            return 0;
        }
        return 0; // Behave like Linux: 0 can also mean “no more for now”
    }

    // 2) Serialize struct list into user buffer
    let out = unsafe { core::slice::from_raw_parts_mut(user_buf, buf_len) };
    let mut off = 0usize;

    for it in &items {
        // header
        let hdr = Dirent64Hdr {
            d_ino: it.ino,
            d_off: it.next_cookie,
            d_reclen: it.reclen,
            d_type: it.dtype,
        };
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(
                (&hdr as *const Dirent64Hdr) as *const u8,
                size_of::<Dirent64Hdr>(),
            )
        };
        out[off..off + hdr_bytes.len()].copy_from_slice(hdr_bytes);
        off += hdr_bytes.len();

        // name + NUL
        let nb = it.name.as_bytes();
        out[off..off + nb.len()].copy_from_slice(nb);
        off += nb.len();
        out[off] = 0;
        off += 1;

        // padding to 8 bytes (reclen accounts for it)
        let pad = (it.reclen as usize) - (size_of::<Dirent64Hdr>() + nb.len() + 1);
        if pad > 0 {
            for b in &mut out[off..off + pad] {
                *b = 0;
            }
            off += pad;
        }

        idx += 1; // consumed this entry
    }

    // 3) Advance directory position
    file.seek = idx;

    off as i64
}

pub(crate) fn stat(file_name_ptr: usize, stat_ptr: usize) -> i64 {
    let file_name_ptr = UserPtr(file_name_ptr as *const u8);
    let Ok(mut file_path) = copy_cstr_from_user(file_name_ptr, 4096) else {
        return -1;
    };

    if file_path.starts_with("./") {
        #[allow(static_mut_refs)]
        let pwd = unsafe { DIR.as_str() };
        let calnonical_pwd = if pwd.ends_with("/") {
            pwd.to_string()
        } else {
            format!("{}/", pwd)
        };
        file_path = file_path.replace("./", &calnonical_pwd.as_str());
    }

    #[allow(static_mut_refs)]
    let Ok(metadata) = (unsafe { VFS.get_mut().metadata(&file_path) }) else {
        return -1;
    };

    let user_stat = unsafe { &mut *(stat_ptr as *mut Stat) };

    user_stat.st_size = metadata.size as i64;
    user_stat.st_mode = match metadata.file_type {
        FileType::File => 0o100644,        // regular file: rw-r--r--
        FileType::Dir => 0o040755,         // directory: rwxr-xr-x
        FileType::CharDevice => 0o020666,  // char device: rw-rw-rw-
        FileType::BlockDevice => 0o060660, // block device: rw-rw----
    };
    user_stat.st_uid = 0;
    user_stat.st_gid = 0;
    user_stat.st_ino = metadata.ino as u64;
    user_stat.st_nlink = 1;
    user_stat.st_rdev = 0;
    user_stat.st_atim = Timespec {
        tv_sec: metadata.access_time as i64,
        tv_nsec: 0,
    };
    user_stat.st_ctim = Timespec {
        tv_sec: metadata.created_time as i64,
        tv_nsec: 0,
    };
    user_stat.st_mtim = Timespec {
        tv_sec: metadata.modified_time as i64,
        tv_nsec: 0,
    };

    0
}

pub fn fstat(fd: usize, fstat_ptr: usize) -> i64 {
    if fstat_ptr == 0 {
        return -(EFAULT as i64);
    }

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    let user_stat = unsafe { &mut *(fstat_ptr as *mut Stat) };
    *user_stat = Stat::default();

    if fd <= 2 {
        let now = uptime() as i64;
        user_stat.st_mode = 0o020666;
        user_stat.st_uid = 0;
        user_stat.st_gid = 0;
        user_stat.st_ino = fd as u64;
        user_stat.st_nlink = 1;
        user_stat.st_size = 0;
        user_stat.st_blksize = 4096;
        user_stat.st_blocks = 0;
        let ts = Timespec {
            tv_sec: now,
            tv_nsec: 0,
        };
        user_stat.st_atim = ts;
        user_stat.st_mtim = ts;
        user_stat.st_ctim = ts;
        return 0;
    }

    if fd > i32::MAX as usize {
        return -(EBADF as i64);
    }

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    let metadata = {
        let file = file_ref.lock();
        let node = file.node.lock();
        node.metadata.clone()
    };

    user_stat.st_size = metadata.size as i64;
    user_stat.st_mode = match metadata.file_type {
        FileType::File => 0o100644,
        FileType::Dir => 0o040755,
        FileType::CharDevice => 0o020666,
        FileType::BlockDevice => 0o060660,
    };
    user_stat.st_uid = 0;
    user_stat.st_gid = 0;
    user_stat.st_ino = metadata.ino as u64;
    user_stat.st_nlink = 1;
    user_stat.st_rdev = 0;
    user_stat.st_blksize = 4096;
    user_stat.st_blocks = ((metadata.size as u64 + 511) / 512) as i64;
    user_stat.st_atim = Timespec {
        tv_sec: metadata.access_time as i64,
        tv_nsec: 0,
    };
    user_stat.st_mtim = Timespec {
        tv_sec: metadata.modified_time as i64,
        tv_nsec: 0,
    };
    user_stat.st_ctim = Timespec {
        tv_sec: metadata.created_time as i64,
        tv_nsec: 0,
    };

    0
}

pub fn getcwd(buf_ptr: usize, buf_len: usize) -> i64 {
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
    #[allow(static_mut_refs)]
    let proc = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let cwd = proc.pwd.as_str();
    let cwd_bytes = cwd.as_bytes();
    buf[..cwd_bytes.len()].copy_from_slice(cwd_bytes);

    buf.as_ptr() as i64
}

pub fn chdir(path_ptr: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -1;
    };

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let dir_path = if path.starts_with("./") || !path.starts_with("/") {
        #[allow(static_mut_refs)]
        let pwd = process.pwd.as_str();
        let calnonical_pwd = if pwd.ends_with("/") {
            pwd.to_string()
        } else {
            format!("{}/", pwd)
        };
        format!("{}{}", calnonical_pwd, path.replace("./", ""))
    } else {
        path
    };

    let dir_path = if dir_path.ends_with("..") {
        let parts = dir_path.split("/");
        let mut vec = parts.collect::<Vec<&str>>();

        vec.pop();
        vec.pop();

        if vec.is_empty() || (vec[0] == "" && vec.len() == 1) {
            "/".to_string()
        } else {
            vec.join("/")
        }
    } else {
        dir_path
    };

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(inode) = fs.open(dir_path.as_str()) {
        if inode.metadata.file_type != FileType::Dir {
            return -1;
        }

        process.pwd = dir_path;
        0
    } else {
        -1
    }
}

pub fn unlink(path_ptr: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -1;
    };

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    // Resolve full path
    let full_path = if path.starts_with('/') {
        normalize_path(path.as_str())
    } else {
        format!("{}/{}", process.pwd.as_str(), path)
    };

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(mut inode) = fs.open(full_path.as_str()) {
        inode.unlink().unwrap() as i64
    } else {
        0
    }
}

pub fn lseek(fd: usize, offset: u64, whence: u8) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    if fd < 3 {
        return -(EBADF as i64);
    }

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    match whence {
        0 => {
            file.seek = offset as usize;
            file.seek as i64
        }
        1 => file.seek as i64,
        2 => {
            let size = file.node.lock().metadata.size;
            file.seek = size;
            file.seek as i64
        }
        _ => -(EINVAL as i64),
    }
}

pub fn readv(fd: usize, iov_ptr: u64, iov_count: u64) -> i64 {
    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, iov_count as usize) };

    let mut total: i64 = 0;

    for iv in iov {
        // Skip empty segments
        if iv.iov_len == 0 {
            continue;
        }

        let buf = unsafe { core::slice::from_raw_parts_mut(iv.iov_base as *mut u8, iv.iov_len) };

        // Write this segment
        let r = read(fd, buf);
        total = total.saturating_add(r);

        // Stop on partial write (short write semantics)
        if (r as usize) < iv.iov_len {
            break;
        }
    }

    total
}

pub fn ioctl(fd: usize, cmd: usize, arg: usize) -> i64 {
    if fd <= 2 {
        let tty = get_tty();

        return tty.ioctl(&mut dummy_blockdev(), cmd as u64, arg).unwrap();
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap_unchecked()
            .get_process(crate::sys::proc::id())
            .unwrap_unchecked()
    };

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    file_ref.lock().node.lock().ioctl(cmd as u64, arg).unwrap()
}

pub fn utimenat(dirfd: i32, str_ptr: usize, _time_ptr: usize, _flags: usize) -> i64 {
    if dirfd != -100 {
        return -1;
    }

    let usr_ptr = UserPtr(str_ptr as *const u8);

    let Ok(path) = copy_cstr_from_user(usr_ptr, 4096) else {
        return -(EFAULT as i64);
    };

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut_unchecked()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let can_path = if path.starts_with("/") {
        path
    } else {
        format!("{}/{}", process.pwd, path)
    };

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    let Ok(_node) = fs.open(can_path.as_str()) else {
        return -ENOENT as i64;
    };

    0
}

pub fn mkdir(path_str: usize, _mode: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_str as *const u8), 4096) else {
        return -1;
    };

    let can_path = format_path(path);

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(_node) = fs.open(can_path.as_str()) {
        return -EEXIST as i64;
    };

    let parent_path = parent_path(can_path.as_str());
    let dir_name = can_path.split("/").last().unwrap();

    if let Ok(_) = fs.mkdir(parent_path, dir_name) {
        0
    } else {
        -1
    }
}

pub fn rmdir(path_str: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_str as *const u8), 4096) else {
        return -1;
    };
    let can_path = format_path(path);
    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(_) = fs.rmdir(can_path.as_str()) {
        0
    } else {
        -1
    }
}

pub fn setuid(uid: u64) -> i64 {
    sys::proc::user::set_uid(uid as usize);
    sys::proc::user::set_user_env();

    0
}

fn poll_fd_set(fds: &mut [PollFd], process: &mut Process) -> Result<usize, i64> {
    let mut ready_count = 0;

    for pfd in fds.iter_mut() {
        pfd.revents = 0;
        let fd = pfd.fd;
        if fd < 0 {
            continue;
        }

        let want_in = (pfd.events & POLLIN) != 0;
        let want_out = (pfd.events & POLLOUT) != 0;
        let mut revents: i16 = 0;

        match fd {
            0 => {
                if want_in {
                    let tty = get_tty();
                    let mut dev = dummy_blockdev();
                    match tty.poll(&mut dev) {
                        Ok(true) => revents |= POLLIN,
                        Ok(false) => {}
                        Err(_) => revents |= POLLERR,
                    }
                }
                if want_out {
                    revents |= POLLOUT;
                }
            }
            1 | 2 => {
                if want_out {
                    revents |= POLLOUT;
                }
            }
            _ => {
                if fd < 3 {
                    pfd.revents = POLLNVAL;
                    ready_count += 1;
                    continue;
                }
                let file_ref = match clone_open_file(process, fd) {
                    Ok(f) => f,
                    Err(_) => {
                        pfd.revents = POLLNVAL;
                        ready_count += 1;
                        continue;
                    }
                };
                let guard_ref = file_ref.lock();
                let mut node = guard_ref.node.lock();
                if want_in {
                    match node.poll() {
                        Ok(true) => revents |= POLLIN,
                        Ok(false) => {}
                        Err(_) => revents |= POLLERR,
                    }
                }
                if want_out {
                    revents |= POLLOUT;
                }
            }
        }

        if revents != 0 {
            pfd.revents = revents;
            ready_count += 1;
        }
    }

    Ok(ready_count)
}

pub fn poll(fds_ptr: usize, nfds: usize, timeout_ms: isize) -> i64 {
    if nfds == 0 {
        return 0;
    }
    if fds_ptr == 0 {
        return -(EFAULT as i64);
    }

    let fds = unsafe { core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, nfds) };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let mut ready = match poll_fd_set(fds, process) {
        Ok(n) => n,
        Err(e) => return e,
    };

    if ready > 0 {
        return ready as i64;
    }
    if timeout_ms == 0 {
        return 0;
    }

    let infinite = timeout_ms < 0;
    let start = uptime();
    let deadline = if infinite {
        None
    } else {
        Some(start + (timeout_ms as f64) / 1000.0)
    };

    loop {
        if let Some(limit) = deadline {
            if uptime() >= limit {
                return 0;
            }
        }

        halt();

        ready = match poll_fd_set(fds, process) {
            Ok(n) => n,
            Err(e) => return e,
        };

        if ready > 0 {
            return ready as i64;
        }
    }
}

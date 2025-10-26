use crate::arch::x86_64::io::{rdmsr, wrmsr, IA32_FS_BASE, IA32_GS_BASE};
use crate::driver::disk::dummy_blockdev;
use crate::sys::console::{get_tty, DIR};
use crate::sys::fs::vfs::{FileType, VfsNodeOps, VFS};
use crate::sys::proc::{Handler, Process, PROCESS_TABLE, USER_STACK_SIZE};
use crate::sys::syscall::utils::{copy_cstr_from_user, copy_user_ptr_array, UserPtr};
use crate::print;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;
use spin::mutex::Mutex;
use twilight_common::syscall::types::*;

pub fn write(arg1: i32, arg2: usize, arg3: usize) -> i64 {
    let file_descriptor = arg1;
    let buf = arg2 as *const u8;
    let len = arg3;
    let buf = unsafe { core::slice::from_raw_parts(buf, len) };

    let res = match file_descriptor {
        1 => {
            print!("{}", String::from_utf8_lossy(buf));

            len as i64
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

            if let Some(node) = process.handler.get_mut(n as usize - 3) {
                if let Ok(_) = node.handler.lock().write(buf) {
                    return len as i64;
                }
            }

            -1
        }
    };

    res
}

pub fn read(handler: usize, buf: &mut [u8]) -> i64 {

    if handler == 0 || handler <= 2 {
        let tty = get_tty();
        if let Ok(v) = tty.read(&mut dummy_blockdev(), 0, buf) {
            return v.len() as i64;
        }

        return 0;
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    if let Some(node) = process.handler.get_mut(handler - 3) {
        if node.handler.lock().metadata.file_type == FileType::Dir {
            return -EISDIR as i64;
        }
        let seek = node.seek;
        if let Ok(content) = node.handler.lock().read() {
            let copy_len = if seek < content.len() {
                (content.len() - seek).min(buf.len())
            } else {
                0
            };
            if copy_len > 0 {
                buf[..copy_len].copy_from_slice(&content[seek..(seek + copy_len)]);
            }
            node.seek += copy_len;
            return copy_len as i64;
        }
    }

    -1
}

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

fn base_for_dirfd(process: &mut Process, dirfd: i32) -> Result<String, i32> {
    if dirfd == AT_FDCWD {
        return Ok(process.pwd.clone());
    }
    if dirfd < 3 {
        return Err(-EBADF);
    }
    let idx = (dirfd - 3) as usize;
    if idx >= process.handler.len() {
        return Err(-EBADF);
    }

    // You store '&'static mut Handler' in the table:
    let h: &mut Handler = process.handler[idx];

    // Ensure it’s a directory FD
    if h.handler.lock().metadata.file_type != FileType::Dir {
        return Err(-ENOTDIR);
    }

    Ok(h.path.clone())
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

    // Install FD
    let new_fd = process.handler.len() + 3;
    let h = Box::leak(Box::new(Handler {
        handler: Arc::new(Mutex::new(node)),
        seek: 0,
        path: full_path,
        flags, // <-- store flags so read/write can enforce later
    }));
    process.handler.push(h);
    new_fd as i64
}

pub fn execev(arg1: usize, arg2: usize, _arg3: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(arg1 as *const u8), 4096) else {
        return -1;
    };
    #[allow(static_mut_refs)]
    let Ok(mut elf_node) = (unsafe { VFS.read().open(path.as_str()) }) else {
        return -2;
    };

    let Ok(elf_buf) = elf_node.read() else {
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
            fill(&mut uname_s.version, "#1 NON-SMP 12-10-2025");
            fill(&mut uname_s.machine, "x86_64");
            fill(&mut uname_s.domainname, "-");
        }
    }

    0
}

pub fn arch_prctl(code: u64, addr: u64) -> i64 {
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
        _ => -1,
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
        total = total.saturating_add(r);

        // Stop on partial write (short write semantics)
        if (r as usize) < iv.iov_len {
            break;
        }
    }
    total
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
    if fd < 3 {
        return -(EBADF as i64);
    }

    let h = match process.handler.get_mut(fd as usize - 3) {
        Some(h) => h,
        None => return -(EBADF as i64),
    };

    if h.handler.lock().metadata.file_type != FileType::Dir {
        return -(ENOTDIR as i64);
    }

    // Read directory entries from VFS (adjust API if yours differs)
    #[allow(static_mut_refs)]
    let entries = match unsafe { VFS.get_mut().ls(&h.path) } {
        Ok(v) => v, // Vec<DirEntry { name:String, inode:u64, file_type:FileType }>
        Err(_) => return -(EIO as i64),
    };

    // Current position (use as entry index 'cookie')
    let mut idx = h.seek;
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
    h.seek = idx;

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

pub fn fstat(_fd: usize, _fstat_ptr: usize) -> i64 {
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

    if let Ok(inode) = fs.open(full_path.as_str()) {
        let handler = Handler {
            handler: Arc::new(Mutex::new(inode)),
            seek: 0,
            path,
            flags: 0, // <-- store flags so read/write can enforce later
        };
        handler.handler.lock().unlink().unwrap() as i64
    } else {
        0
    }
}

pub fn lseek(fd: usize, offset: u64, seek: u8) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };


    if let Some(node) = process.handler.get_mut(fd - 3) {
        match seek {
            0 => {
                node.seek = offset as usize;
                0
            }
            1 => {
                node.seek as i64
            }
            2 => {
                node.seek = node.handler.lock().metadata.size;
                node.seek as i64
            }
            _ => {
                -1
            }
        }
    } else {
        -1
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

    0
}
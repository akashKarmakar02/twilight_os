pub const ARCH_SET_GS: u64 = 0x1001;
pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;
pub const ARCH_GET_GS: u64 = 0x1004;

// --- AT_* ---
pub const AT_FDCWD: i32 = -100;

// --- errno (positive; return -ERR as i64) ---
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const EBADF: i32 = 9;
pub const EAGAIN: i32 = 11;
pub const EEXIST: i32 = 17;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const EOPNOTSUPP: i32 = 95;

pub const O_RDONLY: i32 = 0;
pub const O_WRONLY: i32 = 1;
pub const O_RDWR: i32 = 2;
pub const O_ACCMODE: i32 = 3;

pub const O_CREAT: i32 = 0o100; // 64
pub const O_EXCL: i32 = 0o200; // 128
pub const O_TRUNC: i32 = 0o1000; // 512
pub const O_APPEND: i32 = 0o2000; // 1024
pub const O_NONBLOCK: i32 = 0o4000; // 2048
pub const O_DIRECTORY: i32 = 0o200000; // 65536
pub const O_NOFOLLOW: i32 = 0o400000; // 131072
pub const O_CLOEXEC: i32 = 0o2000000; // 524288
pub const O_PATH: i32 = 0o10000000; // 2097152

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Iovec {
    pub iov_base: *const u8,
    pub iov_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 0x0001;
pub const POLLPRI: i16 = 0x0002;
pub const POLLOUT: i16 = 0x0004;
pub const POLLERR: i16 = 0x0008;
pub const POLLHUP: i16 = 0x0010;
pub const POLLNVAL: i16 = 0x0020;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,  // time_t: seconds
    pub tv_nsec: i64, // long: nanoseconds
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

pub type Rlim = u64;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Rlimit64 {
    pub rlim_cur: Rlim, // soft limit
    pub rlim_max: Rlim, // hard limit
}

pub const RLIM64_INFINITY: Rlim = u64::MAX;

// resource selectors (same numeric values as Linux)
pub const RLIMIT_CPU: u32 = 0;
pub const RLIMIT_FSIZE: u32 = 1;
pub const RLIMIT_DATA: u32 = 2;
pub const RLIMIT_STACK: u32 = 3; // <-- the one Zig/musl touch
pub const RLIMIT_CORE: u32 = 4;
pub const RLIMIT_RSS: u32 = 5; // ignored on Linux
pub const RLIMIT_NPROC: u32 = 6;
pub const RLIMIT_NOFILE: u32 = 7;
pub const RLIMIT_MEMLOCK: u32 = 8;
pub const RLIMIT_AS: u32 = 9;
pub const RLIMIT_LOCKS: u32 = 10;
pub const RLIMIT_SIGPENDING: u32 = 11;
pub const RLIMIT_MSGQUEUE: u32 = 12;
pub const RLIMIT_NICE: u32 = 13;
pub const RLIMIT_RTPRIO: u32 = 14;
pub const RLIMIT_RTTIME: u32 = 15;

#[repr(C, packed)]
pub struct Dirent64Hdr {
    pub d_ino: u64,    // inode
    pub d_off: i64,    // cookie to next entry
    pub d_reclen: u16, // total size of this record
    pub d_type: u8,    // DT_*
                       // d_name[] follows (NUL-terminated), then padding to 8-byte boundary
}

pub const EFAULT: u32 = 14;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Stat {
    pub st_dev_id: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atim: Timespec,
    pub st_mtim: Timespec,
    pub st_ctim: Timespec,
    pub __unused: [i64; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct FStat {
    pub st_dev: u64,        // ID of device containing file
    pub st_ino: u64,        // Inode number
    pub st_mode: u32,       // File type and mode
    pub st_nlink: u32,      // Number of hard links
    pub st_uid: u32,        // User ID of owner
    pub st_gid: u32,        // Group ID of owner
    pub st_rdev: u64,       // Device ID (if special file) else 0
    pub st_size: i64,       // Total size, in bytes
    pub st_blksize: i64,    // Block size for filesystem I/O
    pub st_blocks: i64,     // Number of allocated 512B blocks
    pub st_atime: Timespec, // Time of last access
    pub st_mtime: Timespec, // Time of last modification
    pub st_ctime: Timespec, // Time of last status change
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum Seek {
    Set,
    Cur,
    End,
}

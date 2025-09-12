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
pub struct Iovec {
    pub iov_base: *const u8,
    pub iov_len: usize,
}

#[repr(C, packed)]
#[derive(Debug)]
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
pub const RLIMIT_CPU:        u32 = 0;
pub const RLIMIT_FSIZE:      u32 = 1;
pub const RLIMIT_DATA:       u32 = 2;
pub const RLIMIT_STACK:      u32 = 3; // <-- the one Zig/musl touch
pub const RLIMIT_CORE:       u32 = 4;
pub const RLIMIT_RSS:        u32 = 5; // ignored on Linux
pub const RLIMIT_NPROC:      u32 = 6;
pub const RLIMIT_NOFILE:     u32 = 7;
pub const RLIMIT_MEMLOCK:    u32 = 8;
pub const RLIMIT_AS:         u32 = 9;
pub const RLIMIT_LOCKS:      u32 = 10;
pub const RLIMIT_SIGPENDING: u32 = 11;
pub const RLIMIT_MSGQUEUE:   u32 = 12;
pub const RLIMIT_NICE:       u32 = 13;
pub const RLIMIT_RTPRIO:     u32 = 14;
pub const RLIMIT_RTTIME:     u32 = 15;

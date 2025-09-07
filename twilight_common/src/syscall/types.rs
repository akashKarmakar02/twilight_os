pub const ARCH_SET_GS: u64 = 0x1001;
pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;
pub const ARCH_GET_GS: u64 = 0x1004;

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

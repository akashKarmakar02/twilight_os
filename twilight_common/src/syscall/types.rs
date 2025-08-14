#[repr(C, packed)]
#[derive(Debug)]
pub struct Timespec {
    pub tv_sec: i64,  // time_t: seconds
    pub tv_nsec: i64, // long: nanoseconds
}

use core::ffi::c_void;
use core::ffi::c_int;
use core::ffi::c_char;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe {
        let mut i = 0;
        while i < n {
            *dest.add(i) = *src.add(i);
            i += 1;
        }
        dest
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    unsafe {
        if src < dest as *const u8 {
            // copy from end
            let mut i = n;
            while i > 0 {
                i -= 1;
                *dest.add(i) = *src.add(i);
            }
        } else {
            // copy from beginning
            let mut i = 0;
            while i < n {
                *dest.add(i) = *src.add(i);
                i += 1;
            }
        }
        dest
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut u8, c: c_int, n: usize) -> *mut u8 {
    unsafe {
        let mut i = 0;
        while i < n {
            *dest.add(i) = c as u8;
            i += 1;
        }
        dest
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> c_int {
    unsafe {
        let mut i = 0;
        while i < n {
            let a = *s1.add(i);
            let b = *s2.add(i);
            if a != b {
                return a as c_int - b as c_int;
            }
            i += 1;
        }
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(s1: *const u8, s2: *const u8, n: usize) -> c_int {
    unsafe {
        memcmp(s1, s2, n)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strlen(s: *const c_char) -> usize {
    unsafe {
        let mut i = 0;
        while *s.add(i) != 0 {
            i += 1;
        }
        i
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

#[unsafe(no_mangle)]
pub extern "C" fn _Unwind_Resume() {
    loop {}
}

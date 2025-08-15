pub mod numbers;
pub mod types;

use core::arch::asm;

#[doc(hidden)]
pub unsafe fn syscall0(n: usize) -> usize {
    unsafe {
        let res: usize;
        asm!(
        "int 0x80", in("rax") n,
        lateout("rax") res
        );
        res
    }
}

#[doc(hidden)]
pub unsafe fn syscall1(n: usize, arg1: usize) -> usize {
    unsafe {
        let res: usize;
        asm!(
        "int 0x80", in("rax") n,
        in("rdi") arg1,
        lateout("rax") res
        );
        res
    }
}

#[doc(hidden)]
pub unsafe fn syscall2(n: usize, arg1: usize, arg2: usize) -> usize {
    unsafe {
        let res: usize;
        asm!(
        "int 0x80", in("rax") n,
        in("rdi") arg1, in("rsi") arg2,
        lateout("rax") res
        );
        res
    }
}

#[doc(hidden)]
pub unsafe fn syscall3(n: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    unsafe {
        let res: usize;
        asm!(
        "int 0x80", in("rax") n,
        in("rdi") arg1, in("rsi") arg2, in("rdx") arg3,
        lateout("rax") res
        );
        res
    }
}

#[doc(hidden)]
pub unsafe fn syscall4(n: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    unsafe {
        let res: usize;
        asm!(
        "int 0x80", in("rax") n,
        in("rdi") arg1, in("rsi") arg2, in("rdx") arg3, in("r10") arg4,
        lateout("rax") res
        );
        res
    }
}

#[doc(hidden)]
pub unsafe fn syscall5(n: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> usize {
    unsafe {
        let res: usize;
        asm!(
        "int 0x80", in("rax") n,
        in("rdi") arg1, in("rsi") arg2, in("rdx") arg3, in("r10") arg4, in("r8") arg5,
        lateout("rax") res
        );
        res
    }
}

#[macro_export]
macro_rules! syscall {
    ($n:expr) => {
        $crate::syscall::syscall0($n as usize)
    };
    ($n:expr, $a1:expr) => {
        $crate::syscall::syscall1($n as usize, $a1 as usize)
    };
    ($n:expr, $a1:expr, $a2:expr) => {
        $crate::syscall::syscall2($n as usize, $a1 as usize, $a2 as usize)
    };
    ($n:expr, $a1:expr, $a2:expr, $a3:expr) => {
            $crate::syscall::syscall3($n as usize, $a1 as usize, $a2 as usize, $a3 as usize)
    };
    ($n:expr, $a1:expr, $a2:expr, $a3:expr, $a4:expr) => {
        $crate::syscall::syscall4(
            $n as usize,
            $a1 as usize,
            $a2 as usize,
            $a3 as usize,
            $a4 as usize,
        )
    };
    ($n:expr, $a1:expr, $a2:expr, $a3:expr, $a4:expr, $a5:expr) => {
        $crate::syscall::syscall5(
            $n as usize,
            $a1 as usize,
            $a2 as usize,
            $a3 as usize,
            $a4 as usize,
            $a5 as usize,
        )
    }
}

use crate::println;
use core::arch::{asm};
use crate::sys::proc::task::{task_spinup, Context};

static mut TASK_A_STACK: [u8; 4096] = [0; 4096];
static mut TASK_B_STACK: [u8; 4096] = [0; 4096];
static mut TASK_A_DONE: bool = false;
static mut TASK_B_DONE: bool = false;

static mut TASK_A_CTX_SP: *mut Context = core::ptr::null_mut();
static mut TASK_B_CTX_SP: *mut Context = core::ptr::null_mut();
static mut BOOT_CTX_SP: *mut Context = core::ptr::null_mut();

extern "C" fn task_a() -> ! {
    for i in 0..10 {
        println!("[A] tick {i}");
        #[allow(static_mut_refs)]
        if unsafe { !TASK_B_DONE } {
            unsafe { yield_to(&mut TASK_B_CTX_SP, &mut TASK_A_CTX_SP) };
        }
    }
    println!("[A] done");
    #[allow(static_mut_refs)]
    unsafe { task_exit(&mut TASK_A_DONE, &mut TASK_A_CTX_SP) };
}

extern "C" fn task_b() -> ! {
    for i in 0..5 {
        println!("[B] tick {i}");
        #[allow(static_mut_refs)]
        if unsafe { !TASK_A_DONE } {
            unsafe { yield_to(&mut TASK_A_CTX_SP, &mut TASK_B_CTX_SP) };
        }
    }
    println!("[B] done");
    #[allow(static_mut_refs)]
    unsafe { task_exit(&mut TASK_B_DONE, &mut TASK_B_CTX_SP) };
}

#[inline(never)]
fn yield_to(next_sp: &mut *mut Context, my_sp_slot: &mut *mut Context) {
    unsafe { task_spinup(my_sp_slot, *next_sp) }; 
}

// Cooperative scheduler: pick next runnable or return to boot if none.
#[inline(never)]
fn schedule(from_slot: &mut *mut Context) -> ! {
    // Decide who "I" am based on which slot pointer we got.
    // NOTE: comparing addresses of the globals is enough in this tiny demo.
    #[allow(static_mut_refs)]
    let me_is_a = (from_slot as *mut _) == (unsafe { &mut TASK_A_CTX_SP as *mut _ });

    // Choose target
    let target = if unsafe { !TASK_A_DONE || !TASK_B_DONE } {
        // someone still runnable
        if me_is_a {
            if unsafe { !TASK_B_DONE } { 
                unsafe { TASK_B_CTX_SP }
            } else { 
                unsafe { BOOT_CTX_SP } 
            }
        } else {
            if unsafe { !TASK_A_DONE } { 
                unsafe { TASK_A_CTX_SP }
            } else {
                unsafe { BOOT_CTX_SP }
            }
        }
    } else {
        // both done -> boot
        unsafe { BOOT_CTX_SP }
    };

    unsafe {
        task_spinup(from_slot, target);
        core::hint::unreachable_unchecked()
    }
}

#[inline(never)]
unsafe fn task_exit(my_done_flag: &mut bool, my_sp_slot: &mut *mut Context) -> ! {
    *my_done_flag = true;
    schedule(my_sp_slot) // never returns
}

// Helper: read current CR3 (requires ring0)
pub fn read_cr3() -> u64 {
    let v: u64;
    unsafe { asm!("mov {}, cr3", out(reg) v) };
    v
}

// Place a Context at the *top* of `stack`, with RIP=entry and RBP pointing to stack top.
unsafe fn make_initial_context(stack: &mut [u8], entry: extern "C" fn() -> !) -> *mut Context {
    let cr3 = read_cr3();

    // Compute where the Context will live: at the top of the stack.
    let sp_top = unsafe { stack.as_mut_ptr().add(stack.len()) as usize };
    let ctx_ptr = (sp_top - size_of::<Context>()) as *mut Context;

    // Set up the initial register frame so that pops + `ret` produce a clean call frame.
    unsafe {
        *ctx_ptr = Context {
            cr3,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbx: 0,
            // Set rbp to the actual stack top (after the Context block),
            // so function prologue `push rbp; mov rbp, rsp` is safe.
            rbp: sp_top as u64,
            rip: entry as u64,
        };
    }

    ctx_ptr
}

pub fn switch_demo() {
    unsafe {
        // Build initial “stacks with contexts” for both tasks.

        TASK_A_CTX_SP = make_initial_context(
            {
                #[allow(static_mut_refs)]
                &mut TASK_A_STACK
            },
            task_a,
        );

        TASK_B_CTX_SP = make_initial_context(
            {
                #[allow(static_mut_refs)]
                &mut TASK_B_STACK
            },
            task_b,
        );

        println!("Starting task A…");
        #[allow(static_mut_refs)]
        task_spinup(&mut BOOT_CTX_SP, TASK_A_CTX_SP);

        println!("Returned to kmain");
    }
}

use x86_64::structures::paging::OffsetPageTable;

static mut NEXT_PID: u16 = 1;

// this from https://osblog.stephenmarz.com/ch6.html blog ported to x86_64

// ================== TrapFrame ==================

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
    // general purpose
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // the CPU pushes these automatically on interrupt
    pub rip: usize,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,

    // error code
    pub error_code: u64,
}

// ================== Process ==================

#[repr(C)]
pub enum ProcessState {
    Running,
    Sleeping,
    Waiting,
    Dead,
}

#[repr(C)]
pub struct Process<'a> {
    frame: TrapFrame,
    stack: *mut u8,
    program_counter: usize,
    pid: u16,
    root: *mut OffsetPageTable<'a>,
    state: ProcessState,
}

const KSTACK_SIZE: usize = 4096 * 4;

fn alloc_kstack() -> *mut u8 {
    let boxed = Box::new([0u8, KSTACK_SIZE]);
    Box::into_raw(boxed) as *mut u8;
}

impl<'a> Process<'a> {
    pub fn new(entry_point: usize, root: *mut OffsetPageTable<'a>) -> Self {
        let kstack = alloc_kstack();
        let tf_ptr = unsafe { kstack.add(KSTACK_SIZE - core::mem::size_of::<TrapFrame>()) }
            as *mut TrapFrame;

        unsafe {
            *tf_ptr = TrapFrame {
                r15: 0,
                r14: 0,
                r13: 0,
                r12: 0,
                r11: 0,
                r10: 0,
                r9: 0,
                r8: 0,
                rsi: 0,
                rdi: 0,
                rbp: 0,
                rdx: 0,
                rcx: 0,
                rbx: 0,
                rax: 0,
                rip: entry_point,
                cs: 0x18,         // User code segment selector
                rflags: 0x202,    // IF = 1 (interrupt enabled)
                rsp: 0x8000_0000, // Fake user stack pointer
                ss: 0x23,         // User data segment selector
                error_code: 0,
            };
        }

        let pid = unsafe {
            let id = NEXT_PID;
            NEXT_PID += 1;
            id
        };

        Process {
            frame: unsafe { *tf_ptr },
            stack: kstack,
            program_counter: entry_point,
            pid,
            root,
            state: ProcessState::Running,
        }
    }
}

// ================== Scheduler ==================

use alloc::vec::Vec;
use spin::Mutex;

static mut PROCESS_TABLE: Option<Mutex<Vec<Process>>> = None;
static mut CURRENT_INDEX: usize = 0;

pub fn init_process_table() {
    unsafe {
        PROCESS_TABLE = Some(Mutex::new(Vec::new()));
    }
}

pub fn add_process(process: Process) {
    unsafe {
        if let Some(table) = PROCESS_TABLE {
            table.lock().push(process);
        }
    }
}

pub fn schedule() -> Option<&'static mut Process<'static>> {
    unsafe {
        let table = PROCESS_TABLE.as_mut()?.get_mut();
        if table.is_empty() {
            return None;
        }

        CURRENT_INDEX = (CURRENT_INDEX + 1) % table.len();
        Some(&mut table[CURRENT_INDEX])
    }
}

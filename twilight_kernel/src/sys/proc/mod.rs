use x86_64::structures::paging::OffsetPageTable;

static mut NEXT_PID: u16 = 1;

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


// this from https://osblog.stephenmarz.com/ch6.html blog ported to x86_64
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
    pub r9:  u64,
    pub r8:  u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // the CPU pushes these automatically on interrupt
    pub rip: usize,
    pub cs:  u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss:  u64,

    // error code
    pub error_code: u64,
}

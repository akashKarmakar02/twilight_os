use crate::arch::x86_64::gdt::{USER_CS, USER_SS};
use crate::kernel_utils::exec::jump_to_user;
use crate::println;
use crate::sys::console::init_console;
use crate::sys::memory::{active_level_4_table, alloc_pages, dealloc_pages, frame_allocator, phys_mem_offset};
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU16, Ordering};
use object::{Object, ObjectSegment, SegmentFlags};
use spin::mutex::Mutex;
use spin::Once;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{FrameAllocator, OffsetPageTable, PhysFrame};
use x86_64::VirtAddr;
use crate::arch::x86_64::io::{set_fsbase, set_inactive_gsbase};
use crate::sys::fs::vfs::VfsNode;

pub static mut PROCESS_TABLE: Once<ProcessTable> = Once::new();

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
const STACK_SIZE: usize = 0x4000;
static NEXT_PID: AtomicU16 = AtomicU16::new(1);
static PID: AtomicU16 = AtomicU16::new(0);

// ================== TrapFrame ==================

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
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
    pub rip: usize,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub error_code: u64,
}

// ================== Process ==================

#[repr(C)]
#[derive(Debug)]
pub enum ProcessState {
    Running,
    Sleeping,
    Waiting,
    Dead,
}

pub struct ProcessTable {
    pub proc_list: VecDeque<Process>,
}

unsafe impl Send for ProcessTable {}

impl ProcessTable {
    fn new() -> ProcessTable {
        ProcessTable { proc_list: VecDeque::new() }
    }
}

impl ProcessTable {
    pub fn get_process(&mut self, pid: u16) -> Option<&mut Process> {
        for process in self.proc_list.iter_mut() {
            if process.pid == pid {
                return Some(process);
            }
        }
        None
    }

    pub fn run(&mut self, process: Process) {
        let pid = process.pid;

        PID.store(pid, Ordering::SeqCst);

        self.proc_list.push_back(process);
        self.proc_list.back_mut().unwrap().exec();
    }
}
pub struct Handler {
    pub handler: Arc<Mutex<VfsNode>>,
    pub seek: usize
}
#[repr(C)]
pub struct Process {
    // pub frame: TrapFrame,
    pub stack: u64, // point to user_stack
    pub stack_size: usize,
    pub mapper: OffsetPageTable<'static>,
    pub entry_point: u64,
    pub page_table_frame: PhysFrame,
    pub pid: u16,
    pub state: ProcessState,
    pub addr_size_vec: Vec<(u64, usize)>,
    pub pwd: String,
    pub gs_base: VirtAddr,
    pub fs_base: VirtAddr,
    pub handler: Vec<&'static mut Handler>,
}

impl Process {
    pub fn new(content_buf: Vec<u8>, pwd: &str) -> Self {

        let (_, flags) = Cr3::read();

        let page_table_frame = frame_allocator().allocate_frame().unwrap();

        let page_table = crate::sys::memory::create_page_table(page_table_frame);

        let kernel_page_table = unsafe { active_level_4_table() };

        let pages = page_table.iter_mut().zip(kernel_page_table.iter_mut());

        for (page, kernel_page) in pages {
            *page = kernel_page.clone();
        }

        let mut addr_size_vec: Vec<(u64, usize)> = Vec::new();

        unsafe {
            Cr3::write(page_table_frame, flags);
        };

        let _entry_point_addr: u64 = 0;
        let mut mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        let code_addr = 0x000000000000;
        let user_stack_top = VirtAddr::new(USER_STACK_TOP);

        let mut entry_point_addr: u64 = 0;
        let _phdr_addr: u64 = 0;
        let _phent: u64 = 0;
        let _phnum: u64 = 0;

        let mut ph_count = 0;

        if content_buf.get(0..4) == Some(&ELF_MAGIC) {
            if let Ok(obj) = object::File::parse(content_buf.as_slice()) {
                entry_point_addr = obj.entry();
                ph_count = obj.segments().count();
                for segment in obj.segments() {
                    if let Ok(data) = segment.data() {
                        let addr = code_addr + segment.address();
                        let size = segment.size() as usize;

                        let flags = segment.flags();
                        match flags {
                            SegmentFlags::Elf { p_flags } => {
                                let _is_writable = (p_flags & object::elf::PF_W) != 0;
                                let _is_executable = (p_flags & object::elf::PF_X) != 0;
                                if let Ok(_) = alloc_pages(&mut mapper, addr, size, true, true) {
                                    addr_size_vec.push((addr, size));
                                }
                            }
                            _ => {}
                        }

                        // copy data after allocating
                        let src = data.as_ptr();
                        let dst = addr as *mut u8;
                        unsafe {
                            core::ptr::copy_nonoverlapping(src, dst, data.len());
                            if size > data.len() {
                                core::ptr::write_bytes(dst.add(data.len()), 0, size - data.len());
                            }
                        }
                    }
                }

                let user_stack_base = user_stack_top.as_u64() - STACK_SIZE as u64 ;

                if let Ok(_) = alloc_pages(&mut mapper, user_stack_base, STACK_SIZE, true,false) {
                    addr_size_vec.push((user_stack_base, STACK_SIZE));
                }
            }
        }
        // Some(virt_to_phys(VirtAddr::new(0x400000)).unwrap().as_u64())
        let user_rsp = build_initial_stack(user_stack_top.as_u64(), entry_point_addr, None, None, None, None, Some(ph_count as u64));
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let p = Process {
            stack: user_rsp,
            stack_size: STACK_SIZE,
            entry_point: entry_point_addr,
            pid,
            mapper,
            page_table_frame,
            state: ProcessState::Running,
            addr_size_vec,
            pwd: pwd.to_string(),
            fs_base: VirtAddr::zero(),
            gs_base: VirtAddr::zero(),
            handler: Vec::new()
        };
        p
    }

    pub fn exec(&self) {
        jump_to_user(
            0x000000000000,
            self.entry_point,
            self.stack,
            USER_CS.bits() as u64,
            USER_SS.bits() as u64,
        );
    }

    pub fn cleanup(&mut self) {
        for (addr, size) in self.addr_size_vec.iter() {
            let addr = *addr;
            let size = *size;
            if let Err(_) = dealloc_pages(&mut self.mapper, addr, size) {
                println!("failed to dealloc pages in {:X} of size {}", addr, size);
            }
        }
    }
}

pub fn id() -> u16 {
    PID.load(Ordering::SeqCst)
}

pub fn exit() {
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let mut process = table.proc_list.pop_back().unwrap();

    if let Some(k_process) = table.get_process(0) {
        let page_table_frame = k_process.page_table_frame;
        let (_, flags) = Cr3::read();
        unsafe {
            Cr3::write(page_table_frame, flags);
        }
        set_fsbase()(k_process.fs_base);
        set_inactive_gsbase()(k_process.gs_base);
    }

    process.cleanup();

    init_console();
}

pub fn init() {
    #[allow(static_mut_refs)]
    unsafe {
        PROCESS_TABLE.try_call_once(|| Ok::<_, ()>(ProcessTable::new())).unwrap();
    }
    let (page_table_frame, _) = Cr3::read();
    let page_table = crate::memory::create_page_table(page_table_frame);
    let mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    PID.store(pid, Ordering::SeqCst);

    #[allow(static_mut_refs)]
    unsafe {
        PROCESS_TABLE.get_mut().unwrap().proc_list.push_back(
            Process {
                pid,
                addr_size_vec: Vec::new(),
                stack: 0,
                stack_size: 0,
                entry_point: 0,
                state: ProcessState::Running,
                page_table_frame,
                mapper,
                pwd: "/".to_string(),
                handler: Vec::new(),
                gs_base: VirtAddr::zero(),
                fs_base: VirtAddr::zero(),
            }
        )
    }
}

#[repr(C)]
struct AuxvEntry {
    key: u64,
    value: u64,
}

fn build_initial_stack(
    mut stack_top: u64,
    entry_point: u64,
    argv: Option<&[&str]>,
    envp: Option<&[&str]>,
    phdr_addr: Option<u64>,
    phent: Option<u64>,
    phnum: Option<u64>,
) -> u64 {
    // Keep a vector of pointers (u64) for strings; we'll push them as we write strings.
    let mut string_ptrs: Vec<u64> = Vec::new();

    // Helper: write a byte slice to stack (null-terminated) and return the pointer written.
    fn push_bytes_on_stack(rsp: &mut u64, bytes: &[u8]) -> u64 {
        *rsp = rsp.wrapping_sub((bytes.len() as u64) + 1);
        let dst = *rsp as *mut u8;
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len()) };
        // trailing null
        unsafe { *dst.add(bytes.len()) = 0 };
        *rsp
    }

    // Push env strings (in reverse so final order is preserved)
    if let Some(envs) = envp {
        for &env in envs.iter().rev() {
            let bytes = env.as_bytes();
            let ptr = push_bytes_on_stack(&mut stack_top, bytes);
            string_ptrs.push(ptr);
        }
    }

    // Push argv strings (in reverse)
    if let Some(args) = argv {
        for &arg in args.iter().rev() {
            let bytes = arg.as_bytes();
            let ptr = push_bytes_on_stack(&mut stack_top, bytes);
            string_ptrs.push(ptr);
        }
    }

    // Align stack to 16 bytes (System V: RSP must be aligned to 16 before CALL/IRET)
    if (stack_top & 0xF) != 0 {
        // Make it 16 aligned
        stack_top &= !0xF;
    }

    // Build auxv (values must be u64; convert Option to u64)
    let auxv_entries = [
        AuxvEntry { key: 3, value: phdr_addr.unwrap_or(0) }, // AT_PHDR
        AuxvEntry { key: 4, value: phent.unwrap_or(0) },     // AT_PHENT
        AuxvEntry { key: 5, value: phnum.unwrap_or(0) },     // AT_PHNUM
        AuxvEntry { key: 6, value: 4096 },                  // AT_PAGESZ
        AuxvEntry { key: 9, value: entry_point },           // AT_ENTRY
        AuxvEntry { key: 0, value: 0 },                     // auxv terminator
    ];

    // Push auxv array
    stack_top -= (core::mem::size_of::<AuxvEntry>() * auxv_entries.len()) as u64;
    unsafe {
        let dst = stack_top as *mut AuxvEntry;
        core::ptr::copy_nonoverlapping(auxv_entries.as_ptr(), dst, auxv_entries.len());
    }

    // Null-terminate envp array (a 0 pointer)
    stack_top -= 8;
    unsafe { *(stack_top as *mut u64) = 0; }

    // Push env pointers (if any) in the original order
    if let Some(envs) = envp {
        // string_ptrs currently holds envs then args pushed in that order; we pushed envs first (reversed)
        // To push the env pointers in correct order, pop the last envs.len() entries in reverse.
        for _ in 0..envs.len() {
            stack_top -= 8;
            let ptr = string_ptrs.pop().expect("env pointer expected");
            unsafe { *(stack_top as *mut u64) = ptr; }
        }
    }

    // Null-terminate argv array
    stack_top -= 8;
    unsafe { *(stack_top as *mut u64) = 0; }

    // Push argv pointers (if any)
    if let Some(args) = argv {
        for _ in 0..args.len() {
            stack_top -= 8;
            let ptr = string_ptrs.pop().expect("argv pointer expected");
            unsafe { *(stack_top as *mut u64) = ptr; }
        }
        // argc
        stack_top -= 8;
        unsafe { *(stack_top as *mut u64) = args.len() as u64; }
    } else {
        // no argv: argc = 0
        stack_top -= 8;
        unsafe { *(stack_top as *mut u64) = 0; }
    }

    // Final alignment to 16 bytes before user entry (if necessary)
    if (stack_top & 0xF) != 0 {
        stack_top &= !0xF;
    }

    stack_top
}

//
// #[repr(C)]
// struct AuxvEntry {
//     key: u64,
//     value: u64,
// }
//
// fn build_initial_stack(
//     stack_top: u64,
//     entry_point: u64,
//     argv: Option<&[&str]>,
//     envp: Option<&[&str]>,
//     phdr_addr: Option<u64>,
//     phent: Option<u64>,
//     phnum: Option<u64>
// ) -> u64 {
//     let mut rsp = stack_top;
//
//     let mut string_ptrs: Vec<u64> = Vec::new();
//
//     if let Some(envp) = envp {
//         for &env in envp.iter().rev() {
//             let bytes = env.as_bytes();
//             rsp -= bytes.len() as u64 + 1;
//             unsafe {
//                 core::ptr::copy_nonoverlapping(bytes.as_ptr(), rsp as *mut u8, bytes.len());
//                 *(rsp as *mut u8).add(bytes.len()) = 0;
//             }
//             string_ptrs.push(rsp);
//         }
//     }
//
//     if let Some(argv) = argv {
//         for &arg in argv.iter().rev() {
//             let bytes = arg.as_bytes();
//             rsp -= bytes.len() as u64 + 1;
//             unsafe {
//                 core::ptr::copy_nonoverlapping(bytes.as_ptr(), rsp as *mut u8, bytes.len());
//                 *(rsp as *mut u8).add(bytes.len()) = 0;
//             }
//             string_ptrs.push(rsp);
//         }
//     }
//
//
//     if(rsp & 15) != 0 {
//         rsp -= 8;
//         unsafe { *(rsp as *mut u64) = 0; }
//     }
//
//     let auxv_entries = [
//         AuxvEntry { key: 3, value: phdr_addr }, // AT_PHDR: Program headers address
//         AuxvEntry { key: 4, value: phent }, // AT_PHENT: Program header entry size
//         AuxvEntry { key: 5, value: phnum }, // AT_PHNUM: Number of program headers
//         AuxvEntry { key: 6, value: 4096 }, // AT_PAGEZ: Page size
//         AuxvEntry { key: 9, value: entry_point }, // AT_ENTRY: Entry point
//         AuxvEntry { key: 0, value: 0 }, // NULL terminator
//     ];
//
//     rsp -= (core::mem::size_of::<AuxvEntry>() * auxv_entries.len()) as u64;
//     unsafe {
//         core::ptr::copy_nonoverlapping(
//             auxv_entries.as_ptr(),
//             rsp as *mut AuxvEntry,
//             auxv_entries.len(),
//         );
//     }
//
//     rsp -= 8;
//     unsafe { *(rsp as *mut u64) = 0; }
//
//     for _ in 0..envp.len() {
//         rsp -= 8;
//         unsafe { *(rsp as *mut u64) = string_ptrs.pop().unwrap(); }
//     }
//
//     rsp -= 8;
//     unsafe { *(rsp as *mut u64) = 0; }
//
//     for _ in 0..argv.len() {
//         rsp -= 8;
//         unsafe { *(rsp as *mut u64) = string_ptrs.pop().unwrap(); }
//     }
//
//     rsp -= 8;
//     unsafe { *(rsp as *mut u64) = argv.len() as u64; }
//
//     if (rsp & 15) != 0 {
//         rsp -= 8;
//         unsafe { *(rsp as *mut u64) = 0; }
//     }
//
//     rsp
// }
use crate::arch::x86_64::gdt::{USER_CS, USER_SS};
use crate::kernel_utils::exec::jump_to_user;
use crate::println;
use crate::sys::console::init_console;
use crate::sys::memory::{active_level_4_table, alloc_pages, dealloc_pages, frame_allocator, phys_mem_offset};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use object::{Object, ObjectSegment, SegmentFlags};
use spin::Once;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{FrameAllocator, OffsetPageTable, PhysFrame};
use x86_64::VirtAddr;

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
    pub fn get_process(&self, pid: u16) -> Option<&Process> {
        for process in self.proc_list.iter() {
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

#[repr(C)]
#[derive(Debug)]
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
}


impl Process {
    pub fn new(content_buf: Vec<u8>) -> Self {

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

        let mut entry_point_addr: u64 = 0;
        let mut mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        let code_addr = 0x000000000000;

        if content_buf.get(0..4) == Some(&ELF_MAGIC) {
            if let Ok(obj) = object::File::parse(content_buf.as_slice()) {
                entry_point_addr = obj.entry();

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

                let user_stack_top = VirtAddr::new(USER_STACK_TOP);
                let user_stack_base = user_stack_top.as_u64() - STACK_SIZE as u64 ;

                if let Ok(_) = alloc_pages(&mut mapper, user_stack_base, STACK_SIZE, true,false) {
                    addr_size_vec.push((user_stack_base, STACK_SIZE));
                }
            }
        }

        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let p = Process {
            stack: USER_STACK_TOP,
            stack_size: STACK_SIZE,
            entry_point: entry_point_addr,
            pid,
            mapper,
            page_table_frame,
            state: ProcessState::Running,
            addr_size_vec,
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
            }
        )
    }
}
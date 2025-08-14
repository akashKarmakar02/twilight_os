use crate::sys::memory::{active_level_4_table, alloc_pages, frame_allocator, phys_mem_offset};
use alloc::boxed::Box;
use core::arch::asm;
use core::sync::atomic::{AtomicU16, Ordering};
use x86_64::structures::paging::{FrameAllocator, OffsetPageTable, PageTable};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use object::{Object, ObjectSegment, SegmentFlags};
use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::VirtAddr;
use crate::arch::x86_64::gdt::{USER_CS, USER_SS};
use crate::kernel_utils::exec::jump_to_user;

lazy_static! {
    pub static ref PROCESS_TABLE: Mutex<ProcessTable> =  Mutex::new(ProcessTable::new());
}

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
const STACK_SIZE: usize = 0x4000;
static NEXT_PID: AtomicU16 = AtomicU16::new(1);

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
pub enum ProcessState {
    Running,
    Sleeping,
    Waiting,
    Dead,
}

pub struct ProcessTable {
    pub proc_list: VecDeque<Process>
}

unsafe impl Send for ProcessTable {}

impl ProcessTable {
    fn new() -> ProcessTable {
        ProcessTable { proc_list: VecDeque::new() }
    }
}

impl ProcessTable {

}

#[repr(C)]
pub struct Process {
    // pub frame: TrapFrame,
    pub stack: u64, // point to user_stack
    pub stack_size: usize,
    pub root: OffsetPageTable<'static>,
    pub entry_point: u64,
    pub cr3: PageTable,
    pub pid: u16,
    pub state: ProcessState,
}


impl Process {
    pub fn new(content_buf: Vec<u8>) -> Self {

        // unsafe {
        //     *tf_ptr = TrapFrame {
        //         r15: 0,
        //         r14: 0,
        //         r13: 0,
        //         r12: 0,
        //         r11: 0,
        //         r10: 0,
        //         r9: 0,
        //         r8: 0,
        //         rsi: 0,
        //         rdi: 0,
        //         rbp: 0,
        //         rdx: 0,
        //         rcx: 0,
        //         rbx: 0,
        //         rax: 0,
        //         rip: entry_point,
        //         cs: 0x1B | 3,
        //         rflags: 0x202,
        //         rsp: kstack.add(KSTACK_SIZE) as u64,
        //         ss: 0x23 | 3,
        //         error_code: 0,
        //     };
        // }

        let (_, flags) = Cr3::read();

        let page_table_frame = frame_allocator().allocate_frame().unwrap();

        let page_table = crate::sys::memory::create_page_table(page_table_frame);

        let cloned_page_table = page_table.clone();

        let kernel_page_table = unsafe { active_level_4_table() };

        let pages = page_table.iter_mut().zip(kernel_page_table.iter_mut());

        for (page, kernel_page) in pages {
            *page = kernel_page.clone();
        }

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
                                alloc_pages(&mut mapper, addr, size, true, true).unwrap();
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

                alloc_pages(
                    &mut mapper,
                    user_stack_base,
                    STACK_SIZE as usize,
                    true, // writable
                    false // executable
                ).unwrap();

                // if let Some(phys) = mapper.translate_addr(VirtAddr::new(0x4000c9)) {
                //     println!("Mapped to: {:#x}", phys);
                // } else {
                //     println!("Not mapped!");
                // }
            }
        }

        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);

        Process {
            // frame: unsafe { *tf_ptr },
            stack: USER_STACK_TOP,
            stack_size: STACK_SIZE,
            entry_point: entry_point_addr,
            pid,
            root: mapper,
            cr3: cloned_page_table,
            state: ProcessState::Running,
        }
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
}



// ================== Scheduler ==================

// static PROCESS_TABLE: Once<Mutex<Vec<Process>>> = Once::new();
// static CURRENT_INDEX: AtomicUsize = AtomicUsize::new(0);
//
// pub fn init_process_table() {
//     PROCESS_TABLE.call_once(|| Mutex::new(Vec::new()));
// }
//
// pub fn add_process(process: Process) {
//     if let Some(table) = PROCESS_TABLE.get() {
//         table.lock().push(process);
//     }
// }
//
// pub fn schedule<'a>() -> Option<(&'a mut Process<'a>, u64)> {
//     if let Some(table) = PROCESS_TABLE.get() {
//         let mut table = table.lock();
//         if table.is_empty() {
//             return None;
//         }
//         for _ in 0..table.len() {
//             let index = CURRENT_INDEX.fetch_add(1, Ordering::SeqCst) % table.len();
//             if matches!(table[index].state, ProcessState::Running) {
//                 let process = &mut table[index];
//                 return Some((process, process.cr3));
//             }
//         }
//     }
//     None
// }

pub extern "C" fn context_switch(current: *mut TrapFrame, next: *const TrapFrame, _next_cr3: usize) {
    unsafe {
        asm!(
            // Save registers to current
            "mov [rdi + 0x00], r15",
            "mov [rdi + 0x08], r14",
            "mov [rdi + 0x10], r13",
            "mov [rdi + 0x18], r12",
            "mov [rdi + 0x20], r11",
            "mov [rdi + 0x28], r10",
            "mov [rdi + 0x30], r9",
            "mov [rdi + 0x38], r8",
            "mov [rdi + 0x40], rsi",
            "mov [rdi + 0x48], rdi",
            "mov [rdi + 0x50], rbp",
            "mov [rdi + 0x58], rdx",
            "mov [rdi + 0x60], rcx",
            "mov [rdi + 0x68], rbx",
            "mov [rdi + 0x70], rax",
            // Load registers from next
            "mov r15, [rsi + 0x00]",
            "mov r14, [rsi + 0x08]",
            "mov r13, [rsi + 0x10]",
            "mov r12, [rsi + 0x18]",
            "mov r11, [rsi + 0x20]",
            "mov r10, [rsi + 0x28]",
            "mov r9,  [rsi + 0x30]",
            "mov r8,  [rsi + 0x38]",
            "mov rsi, [rsi + 0x40]",
            "mov rdi, [rsi + 0x48]",
            "mov rbp, [rsi + 0x50]",
            "mov rdx, [rsi + 0x58]",
            "mov rcx, [rsi + 0x60]",
            "mov rbx, [rsi + 0x68]",
            "mov rax, [rsi + 0x70]",
            // Switch page table
            "mov cr3, rdx",
            // Restore stack for iretq
            "mov rsp, [rsi + 0x80]", // rsp
            "push [rsi + 0x90]",     // ss
            "push [rsi + 0x80]",     // rsp
            "push [rsi + 0x88]",     // rflags
            "push [rsi + 0x78]",     // cs
            "push [rsi + 0x70]",     // rip
            "iretq",
            options(noreturn)
        )
    };
}

// pub fn init_user_page_table() -> (OffsetPageTable<'static>, u64) {
//     let mut allocator = SimpleFrameAllocator::new();
//     let mut phys_frame = allocator.allocate_frame().unwrap();
//     let phys_addr = phys_frame.start_address();
//     let virt_addr = VirtAddr::new(phys_addr.as_u64() + phys_mem_offset());
//     let page_table: *mut PageTable = virt_addr.as_mut_ptr();
//
//     unsafe {
//         core::ptr::write_bytes(page_table, 0, 1);
//     }
//
//     let phys_offset = VirtAddr::new(0xffff_8000_0000_0000);
//     (unsafe { OffsetPageTable::new(page_table, phys_offset) }, phys_frame.start_address().as_u64())
// }

// ================== Test Programs ==================
//
// #[naked_asm!]
// pub unsafe extern "C" fn user_program1() {
//     asm!(
//     "mov rax, 1",
//     "mov rdi, 42",
//     "syscall",
//     "jmp $",
//     options(noreturn)
//     );
// }
//
// #[naked_asm!]
// pub unsafe extern "C" fn user_program2() {
//     asm!(
//     "mov rax, 2",
//     "mov rdi, 99",
//     "syscall",
//     "jmp $",
//     options(noreturn)
//     );
// }

// ================== Test ==================

// pub fn test_multitasking() {
//     unsafe { init_gdt() };
//     init_process_table();
//
//     let (mut table1, cr3_1) = init_user_page_table();
//     let (mut table2, cr3_2) = init_user_page_table();
//
//     map_user_program(&mut table1, user_program1 as usize);
//     map_user_program(&mut table2, user_program2 as usize);
//
//     let p1 = Process::new(user_program1 as usize, &mut table1 as *mut _, cr3_1);
//     let p2 = Process::new(user_program2 as usize, &mut table2 as *mut _, cr3_2);
//     add_process(p1);
//     add_process(p2);
//
//     println!("Created processes with PIDs 1 and 2");
//
//     let mut current = Process::new(0, core::ptr::null_mut(), 0);
//     for _ in 0..2 {
//         if let Some((next, cr3)) = schedule() {
//             println!("Switching to process with PID {}", next.pid);
//             unsafe {
//                 context_switch(&mut current.frame, &next.frame, cr3 as usize);
//             }
//         }
//     }
// }




pub fn init() {
    let (page_table, _) = Cr3::read();
    let page_table = crate::memory::create_page_table(page_table);
    let cloned_page_table = page_table.clone();
    let mut mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

    PROCESS_TABLE.lock().proc_list.push_back(
        Process{
            pid: NEXT_PID.fetch_add(1, Ordering::SeqCst),
            stack: 0,
            stack_size: 0,
            entry_point: 0,
            state: ProcessState::Running,
            cr3: cloned_page_table,
            root: mapper,
        }
    )
}
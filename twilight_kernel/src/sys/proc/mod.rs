pub mod mem;
pub mod switch;
mod task;
pub(crate) mod user;

use crate::arch::x86_64::gdt::{SegmentSelector, USER_CS, USER_SS};
use crate::arch::x86_64::io;
use crate::arch::x86_64::io::{IA32_FS_BASE, IA32_GS_BASE, wrmsr};
use crate::kernel_utils::exec::jump_to_user;
use crate::println;
use crate::sys::console::init_console;
use crate::sys::fs::vfs::{VFS, VfsNode};
use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::memory::{alloc_pages, kernel_page_table, phys_mem_offset};
use crate::sys::proc::mem::ProcMM;
use crate::sys::proc::switch::read_cr3;
use crate::sys::proc::task::{
    Context, FpuState, allocate_switch_stack, switch_tasks, xrstor, xsave,
};
use crate::sys::proc::user::USER_ENV;
use crate::utils::StackHelper;
use alloc::alloc::alloc_zeroed;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::arch::naked_asm;
use core::mem::size_of;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicU16, Ordering};
use object::{Object, ObjectSegment, SegmentFlags};
use spin::Once;
use spin::mutex::Mutex;
use twilight_common::syscall::types::{O_RDONLY, O_WRONLY};
use x86_64::VirtAddr;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, PhysFrame, Size4KiB,
};

pub static mut PROCESS_TABLE: Once<ProcessTable> = Once::new();

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
pub const USER_STACK_SIZE: usize = 0x64000;
const MAIN_DYN_LOAD_BASE: u64 = 0x4000_0000;
const INTERP_DYN_LOAD_BASE: u64 = 0x6000_0000;
static NEXT_PID: AtomicU16 = AtomicU16::new(1);
static PID: AtomicU16 = AtomicU16::new(0);
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);
const INITIAL_CONTEXT_STACK_GUARD: u64 = 4096 * 2;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ScratchRegisters {
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct PreservedRegisters {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IretRegisters {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl IretRegisters {
    pub fn is_user(&self) -> bool {
        let selector = SegmentSelector::from_bits(self.cs as u16);
        selector.privilege_level().is_user()
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct InterruptStack {
    pub preserved: PreservedRegisters,
    pub scratch: ScratchRegisters,
    pub iret: IretRegisters,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct InterruptErrorStack {
    pub code: u64,
    pub stack: InterruptStack,
}

#[repr(C, packed)]
#[derive(Debug)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Debug)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;

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
        ProcessTable {
            proc_list: VecDeque::new(),
        }
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

    pub fn run(&mut self, mut process: Process) {
        let pid = process.pid;

        crate::serial_println!(
            "[proc::run] start new pid={} current={} entry={:#x} user_sp={:#x}",
            pid,
            crate::sys::proc::id(),
            process.entry_point,
            process.stack,
        );

        let parent_table_frame = self.proc_list.back().map(|p| p.page_table_frame);
        let (_, cr3_flags) = Cr3::read();

        if let Some(frame) = parent_table_frame {
            // Make sure we save the current task's context with its own page table active.
            unsafe { Cr3::write(frame, cr3_flags) };
        }

        let mut stack_ptr = initial_context_stack_top(process.kernel_gs.kernel_rsp);
        let context_ptr = {
            let mut stack = StackHelper::new(&mut stack_ptr);
            let cr3 = process.page_table_frame.start_address().as_u64() | cr3_flags.bits();

            // Build an initial interrupt frame + context so switch_tasks can iret into userspace.
            let preempt_frame =
                build_initial_preempt_frame(&mut stack, cr3, process.entry_point, process.stack, 0);

            let kframe = stack.offset::<InterruptErrorStack>();
            *kframe = InterruptErrorStack {
                code: 0,
                stack: InterruptStack::default(),
            };
            kframe.stack.iret.ss = USER_SS.bits() as u64;
            kframe.stack.iret.cs = USER_CS.bits() as u64;
            kframe.stack.iret.rip = process.entry_point;
            kframe.stack.iret.rflags = 0x202;
            kframe.stack.iret.rsp = process.stack;

            let context = stack.offset::<Context>();
            *context = Context::default();
            context.rip = iretq_init as u64;
            context.cr3 = cr3;

            process.preempt_frame = preempt_frame as u64;

            context as *mut Context
        };

        process.context_switch_rsp = VirtAddr::new(stack_ptr);
        process.context = context_ptr;

        let current_pid = crate::sys::proc::id();

        PID.store(pid, Ordering::SeqCst);

        self.proc_list.push_back(process);

        let slice = self.proc_list.make_contiguous();
        let len = slice.len();

        let mut prev_idx = None;
        for (i, p) in slice.iter().enumerate() {
            if p.pid == current_pid {
                prev_idx = Some(i);
                break;
            }
        }

        // If we can't find the current process, something is very wrong, but fallback
        // to previous behavior (second to last) might be saf-ish or just panic.
        // For now, let's assume we found it.
        let prev_idx = prev_idx.unwrap_or(len - 2);
        let next_idx = len - 1;

        if prev_idx == next_idx {
            // Should not happen if we pushed a new process
            return;
        }

        let ptr = slice.as_mut_ptr();
        unsafe {
            let prev_task = &mut *ptr.add(prev_idx);
            let next_task = &mut *ptr.add(next_idx);

            // This is a *kernel* context switch (e.g., exec/spawn). The previous task is now blocked
            // in the kernel until the new process exits. Prevent the preemptive timer from resuming it
            // from a stale user preempt frame.
            prev_task.state = ProcessState::Waiting;
            prev_task.preempt_frame = 0;
            next_task.state = ProcessState::Running;

            crate::serial_println!(
                "[proc::run] switch parent pid={} -> child pid={}",
                prev_task.pid,
                next_task.pid,
            );
            switch_tasks(prev_task, next_task);
            crate::serial_println!("[proc::run] returned to pid={}", crate::sys::proc::id());
        }
    }
}
pub struct OpenFile {
    pub kind: OpenFileKind,
    pub seek: usize,
    pub path: String,
    pub status_flags: i32,
}

pub enum OpenFileKind {
    Vfs(Arc<Mutex<VfsNode>>),
    Socket(crate::sys::net::socket::SocketFile),
}

#[derive(Clone)]
pub struct FdEntry {
    pub file: Arc<Mutex<OpenFile>>,
    pub fd_flags: i32,
}

#[repr(C)]
pub struct Process {
    pub context: *mut Context,
    pub context_switch_rsp: VirtAddr,
    pub fpu_storage: Option<FpuState>,
    pub kernel_gs: Box<KernelGsData>,
    pub gs_base: VirtAddr,
    pub fs_base: VirtAddr,

    pub stack: u64, // point to user_stack
    pub stack_size: usize,
    pub mapper: OffsetPageTable<'static>,
    pub entry_point: u64,
    pub page_table_frame: PhysFrame,
    pub pid: u16,
    pub parent_pid: u16,
    pub state: ProcessState,
    pub addr_size_vec: Vec<(u64, usize)>,
    pub pwd: String,
    pub fd_table: Vec<Option<FdEntry>>,
    pub stdio_flags: [i32; 3],
    pub stdio_fd_flags: [i32; 3],
    /// For fd 0/1/2 redirection: -1 means tty, otherwise points to an fd >= 3.
    pub stdio_target: [i32; 3],
    pub proc_mm: Box<ProcMM>,
    pub exit_code: i32,
    pub preempt_frame: u64, // saved RSP to PreemptFrame on this task's kernel stack
}

impl Process {
    pub fn new(
        content_buf: Vec<u8>,
        pwd: &str,
        args: &[&str],
        parent_pid: u16,
    ) -> Result<Self, ()> {
        let (_, flags) = Cr3::read();

        let page_table_frame =
            with_frame_allocator(|frame_allocator| frame_allocator.allocate_frame().unwrap());

        let page_table = crate::sys::memory::create_page_table(page_table_frame);

        let kernel_page_table = kernel_page_table();

        let pages = page_table.iter_mut().zip(kernel_page_table.iter_mut());

        for (_, (page, kernel_page)) in pages.enumerate() {
            *page = kernel_page.clone();
        }

        let mut addr_size_vec: Vec<(u64, usize)> = Vec::new();

        unsafe {
            Cr3::write(page_table_frame, flags);
        };

        let mut mapper =
            unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        let user_stack_top = VirtAddr::new(USER_STACK_TOP);

        let mut entry_point_addr: u64;
        let aux_entry_point: u64;
        let mut at_base: u64 = 0;
        let phdr_va: u64;
        let phent: u64;
        let phnum: u64;
        let mut max_end: u64;

        if content_buf.get(0..4) == Some(&ELF_MAGIC) {
            match load_elf_image(
                content_buf.as_slice(),
                &mut mapper,
                &mut addr_size_vec,
                Some(MAIN_DYN_LOAD_BASE),
                true,
            ) {
                Ok(main_img) => {
                    entry_point_addr = main_img.entry_point;
                    aux_entry_point = entry_point_addr;
                    phdr_va = main_img.phdr_va;
                    phent = main_img.phent;
                    phnum = main_img.phnum;
                    max_end = main_img.max_end;

                    if let Some(interp_path) = main_img.interp_path {
                        match load_interpreter_image(
                            interp_path.as_str(),
                            &mut mapper,
                            &mut addr_size_vec,
                        ) {
                            Ok(interp_img) => {
                                entry_point_addr = interp_img.entry_point;
                                at_base = interp_img.load_base;
                                if interp_img.max_end > max_end {
                                    max_end = interp_img.max_end;
                                }
                            }
                            Err(_) => {
                                println!("ksh: failed to load interpreter {}", interp_path);
                                return Err(());
                            }
                        }
                    }

                    let user_stack_base = user_stack_top.as_u64() - USER_STACK_SIZE as u64;
                    if let Ok(_) =
                        alloc_pages(&mut mapper, user_stack_base, USER_STACK_SIZE, true, false)
                    {
                        addr_size_vec.push((user_stack_base, USER_STACK_SIZE));
                    }
                }
                Err(_) => {
                    println!("ksh: invalid ELF file");
                    return Err(());
                }
            }
        } else {
            println!("ksh: invalid ELF file");
            return Err(());
        }

        let mut env = Vec::new();
        let user_env = USER_ENV.lock();
        for env_part in user_env.iter() {
            env.push(env_part.as_str());
        }

        let user_rsp = build_initial_stack(
            user_stack_top.as_u64(),
            aux_entry_point,
            at_base,
            Some(args),
            Some(env.as_slice()),
            phdr_va,
            phent,
            phnum,
            None,
            None,
        );

        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let proc_mm = Box::new(ProcMM::new(max_end as usize));

        let switch_stack = allocate_switch_stack().unwrap().as_mut_ptr::<u8>();

        let stack_ptr = switch_stack as u64;

        let mut kgs = Box::new(KernelGsData {
            kernel_rsp: 0, // The top of the stack for syscall/interrupt entry
            user_rsp: 0,
        });

        kgs.kernel_rsp = stack_ptr;

        let kgs_va = VirtAddr::new(&*kgs as *const _ as u64);

        let p = Process {
            context: core::ptr::null_mut(), // Point to the constructed context
            context_switch_rsp: VirtAddr::new(stack_ptr), // This field might be redundant if we use context, but keep it consistent
            fpu_storage: Some(FpuState::default()),

            stack: user_rsp,
            stack_size: USER_STACK_SIZE,
            entry_point: entry_point_addr,
            pid,
            mapper,
            page_table_frame,
            state: ProcessState::Running,
            addr_size_vec,
            pwd: pwd.to_string(),
            kernel_gs: kgs,
            fs_base: VirtAddr::zero(),
            gs_base: kgs_va,
            fd_table: Vec::new(),
            proc_mm,
            parent_pid,
            stdio_flags: [O_RDONLY, O_WRONLY, O_WRONLY],
            stdio_fd_flags: [0; 3],
            stdio_target: [-1; 3],
            exit_code: 0,
            preempt_frame: 0,
        };
        Ok(p)
    }

    pub fn exec(
        &mut self,
        content_buf: &[u8],
        args: &[&str],
        env: &[&str],
    ) -> Result<(u64, u64), ()> {
        let (_, flags) = Cr3::read();

        let page_table_frame =
            with_frame_allocator(|frame_allocator| frame_allocator.allocate_frame().unwrap());

        let page_table = crate::sys::memory::create_page_table(page_table_frame);
        let kernel_page_table = kernel_page_table();
        let pages = page_table.iter_mut().zip(kernel_page_table.iter_mut());
        for (_, (page, kernel_page)) in pages.enumerate() {
            *page = kernel_page.clone();
        }

        // We must switch to the new page table to write user data (load ELF, build stack).
        // Since kernel mappings are identical, this is safe for kernel execution.
        // But we must NOT access old user memory after this point until we decide to revert (which we won't on success).
        unsafe {
            Cr3::write(page_table_frame, flags);
        };

        let mut mapper =
            unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        let mut addr_size_vec: Vec<(u64, usize)> = Vec::new();
        let user_stack_top = VirtAddr::new(USER_STACK_TOP);

        let mut entry_point_addr: u64;
        let aux_entry_point: u64;
        let mut at_base: u64 = 0;
        let phdr_va: u64;
        let phent: u64;
        let phnum: u64;
        let mut max_end: u64;

        if content_buf.get(0..4) == Some(&ELF_MAGIC) {
            match load_elf_image(
                content_buf,
                &mut mapper,
                &mut addr_size_vec,
                Some(MAIN_DYN_LOAD_BASE),
                true,
            ) {
                Ok(main_img) => {
                    entry_point_addr = main_img.entry_point;
                    aux_entry_point = entry_point_addr;
                    phdr_va = main_img.phdr_va;
                    phent = main_img.phent;
                    phnum = main_img.phnum;
                    max_end = main_img.max_end;

                    if let Some(interp_path) = main_img.interp_path {
                        // We must read the interpreter file.
                        // We can't use VFS normally if it relies on current process state?
                        // VFS uses `Process::current()`? No, it usually just takes paths.
                        // But accessing "user pointers" in `exec` is tricky if we just switched CR3.
                        // However, `load_interpreter_image` takes a path string (kernel memory), not user pointer.
                        // We should be fine.
                        match load_interpreter_image(
                            interp_path.as_str(),
                            &mut mapper,
                            &mut addr_size_vec,
                        ) {
                            Ok(interp_img) => {
                                entry_point_addr = interp_img.entry_point;
                                at_base = interp_img.load_base;
                                if interp_img.max_end > max_end {
                                    max_end = interp_img.max_end;
                                }
                            }
                            Err(_) => {
                                println!("exec: failed to load interpreter {}", interp_path);
                                // TODO: Revert CR3?
                                return Err(());
                            }
                        }
                    }

                    let user_stack_base = user_stack_top.as_u64() - USER_STACK_SIZE as u64;
                    if alloc_pages(&mut mapper, user_stack_base, USER_STACK_SIZE, true, false)
                        .is_err()
                    {
                        return Err(());
                    }
                    addr_size_vec.push((user_stack_base, USER_STACK_SIZE));
                }
                Err(_) => {
                    println!("exec: invalid ELF file");
                    return Err(());
                }
            }
        } else {
            println!("exec: invalid ELF file");
            return Err(());
        }

        let user_rsp = build_initial_stack(
            user_stack_top.as_u64(),
            aux_entry_point,
            at_base,
            Some(args),
            Some(env),
            phdr_va,
            phent,
            phnum,
            None,
            None,
        );

        let proc_mm = Box::new(ProcMM::new(max_end as usize));

        // Commit changes to self
        // Drop old resources implicitly when overwriting
        self.mapper = mapper;
        self.page_table_frame = page_table_frame;
        self.addr_size_vec = addr_size_vec;
        self.proc_mm = proc_mm;
        self.entry_point = entry_point_addr;
        self.stack = user_rsp;
        self.stack_size = USER_STACK_SIZE; // Reset in case it changed?

        // FPU state reset?
        self.fpu_storage = Some(FpuState::default());

        // File descriptors are PRESERVED (except CLOEXEC, which we handle in syscall service normally, or here?)
        // Standard execve closes CLOEXEC fds.
        for fd in self.fd_table.iter_mut() {
            if let Some(entry) = fd {
                if (entry.fd_flags & 1) != 0 {
                    // FD_CLOEXEC
                    *fd = None;
                }
            }
        }

        // We are already running on the correct kernel stack (sys_execve call stack).
        // self.kernel_gs and context_switch_rsp remain valid for the NEXT trap/interrupt.

        // Return new entry point and stack to caller so they can update the TrapFrame
        Ok((entry_point_addr, user_rsp))
    }

    pub fn exec_wrapper(&self) {
        wrmsr(IA32_FS_BASE, self.fs_base.as_u64());
        wrmsr(IA32_GS_BASE, self.gs_base.as_u64());

        jump_to_user(
            self.entry_point,
            self.stack,
            USER_CS.bits() as u64,
            USER_SS.bits() as u64,
        );
    }

    pub fn fork(&mut self, tf: &InterruptStack) -> Result<Process, ()> {
        let live_fs_base = io::get_fsbase()();
        self.fs_base = live_fs_base;

        // 0. Allocate PID
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        crate::serial_println!(
            "[fork] parent={} child={} rip={:#x} rsp={:#x} regions={} heap={:#x}-{:#x} mmap={}",
            self.pid,
            pid,
            tf.iret.rip,
            tf.iret.rsp,
            self.addr_size_vec.len(),
            self.proc_mm.heap_start,
            self.proc_mm.mapped_heap_end,
            self.proc_mm.mmap_regions.len(),
        );

        // 1. Allocate new page table
        let (_, flags) = Cr3::read();
        let page_table_frame =
            with_frame_allocator(|frame_allocator| frame_allocator.allocate_frame().unwrap());
        let page_table = crate::sys::memory::create_page_table(page_table_frame);
        let kernel_page_table = kernel_page_table();

        // Copy kernel mappings
        let pages = page_table.iter_mut().zip(kernel_page_table.iter_mut());
        for (_, (page, kernel_page)) in pages.enumerate() {
            *page = kernel_page.clone();
        }

        let mut mapper =
            unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        // 2. Deep copy user memory
        let mut regions_to_copy = self.addr_size_vec.clone();

        // Add Heap
        if self.proc_mm.mapped_heap_end > self.proc_mm.heap_start {
            regions_to_copy.push((
                self.proc_mm.heap_start as u64,
                self.proc_mm.mapped_heap_end - self.proc_mm.heap_start,
            ));
        }

        // Add Mmaps
        for region in &self.proc_mm.mmap_regions {
            regions_to_copy.push((region.base as u64, region.len));
        }

        // We use a separate vec for the child's tracking to avoiding duplicates if addr_size_vec used to track everything
        // But for cleanup we need them in child's addr_size_vec.
        let mut child_addr_size_vec = self.addr_size_vec.clone();

        for (addr, size) in regions_to_copy.iter() {
            let addr = *addr;
            let size = *size;
            crate::serial_println!("[fork] copy child={} addr={:#x} size={:#x}", pid, addr, size);

            // Allocate in child
            // Note: We use true, true (RWX) for simplicity, though strict permissions would be better.
            if alloc_pages(&mut mapper, addr, size, true, true).is_err() {
                println!("fork: failed to alloc pages");
                return Err(());
            }

            // Track dynamic allocations in child so they are freed on exit
            // (If already in addr_size_vec, we might duplicate, but cleanup handles that or we should check existence?)
            // addr_size_vec usually has code/data. Heap/Mmap are new.
            // A simple deduplication check:
            if !child_addr_size_vec.contains(&(addr, size)) {
                child_addr_size_vec.push((addr, size));
            }

            let start_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(
                VirtAddr::new(addr),
            );
            let end_page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(
                VirtAddr::new(addr + (size as u64) - 1),
            );

            for page in x86_64::structures::paging::Page::range_inclusive(start_page, end_page) {
                // Get physical address in child's page table
                let phys_opt = mapper.translate_page(page);

                if let Ok(child_frame) = phys_opt {
                    let child_phys = child_frame.start_address();
                    let child_virt = VirtAddr::new(child_phys.as_u64() + phys_mem_offset());
                    let parent_virt = page.start_address();

                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            parent_virt.as_u64() as *const u8,
                            child_virt.as_mut_ptr(),
                            4096,
                        );
                    }
                }
            }
        }

        // 3. Clone File Descriptors
        let mut new_fd_table = Vec::new();
        for fd in self.fd_table.iter() {
            if let Some(entry) = fd {
                new_fd_table.push(Some(entry.clone()));
            } else {
                new_fd_table.push(None);
            }
        }

        // 4. Setup Child Context
        let switch_stack = allocate_switch_stack().unwrap().as_mut_ptr::<u8>();

        // Stack grows down. Point to top.
        let kernel_rsp = switch_stack as u64;
        let mut stack_ptr = initial_context_stack_top(kernel_rsp);

        let kgs = Box::new(KernelGsData {
            kernel_rsp,
            user_rsp: 0,
        });
        let kgs_va = VirtAddr::new(&*kgs as *const _ as u64);

        let mut stack = StackHelper::new(&mut stack_ptr);
        let child_cr3 = page_table_frame.start_address().as_u64() | flags.bits();

        // Allocate space for InterruptErrorStack
        let kframe = stack.offset::<InterruptErrorStack>();

        // Copy parent's trap frame
        *kframe = InterruptErrorStack {
            code: 0,
            stack: *tf,
        };

        // Override RAX to 0 for child (fork returns 0)
        kframe.stack.scratch.rax = 0;

        let context = stack.offset::<Context>();
        *context = Context::default();
        context.rip = iretq_init as u64;
        context.cr3 = child_cr3;

        let child = Process {
            context: context as *mut Context,
            context_switch_rsp: VirtAddr::new(stack_ptr),
            fpu_storage: self.fpu_storage, // Clone FPU state? Yes.

            stack: self.stack, // Copy user stack pointer (same VA)
            stack_size: self.stack_size,
            entry_point: self.entry_point,
            pid,
            mapper,
            page_table_frame,
            state: ProcessState::Running,
            addr_size_vec: child_addr_size_vec,
            pwd: self.pwd.clone(),
            fd_table: new_fd_table,
            kernel_gs: kgs,
            gs_base: kgs_va,
            fs_base: live_fs_base,
            proc_mm: self.proc_mm.clone(), // Need to implement Clone for ProcMM or manually deep copy
            parent_pid: self.pid,
            stdio_flags: self.stdio_flags,
            stdio_fd_flags: self.stdio_fd_flags,
            stdio_target: self.stdio_target,
            exit_code: 0,
            preempt_frame: 0,
        };

        crate::serial_println!("[fork] child={} ready", pid);
        Ok(child)
    }

    pub fn cleanup(&mut self, table_frame: PhysFrame) {
        // The process memory map is not normalized enough yet to safely walk
        // addr_size_vec here. Some regions can overlap with heap/mmap tracking
        // or page-table cleanup, which can corrupt allocator state while reaping
        // a zombie. Keep the old behavior for this multitasking slice: reap the
        // process table entry and release the root page-table frame only.
        self.addr_size_vec.clear();

        with_frame_allocator(|allocator| unsafe {
            allocator.deallocate_frame(table_frame);
        });
    }
}

pub fn id() -> u16 {
    PID.load(Ordering::SeqCst)
}

fn initial_context_stack_top(kernel_rsp: u64) -> u64 {
    kernel_rsp - INITIAL_CONTEXT_STACK_GUARD
}

pub fn exit(code: i32) {
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };

    let current_pid = id();
    crate::serial_println!("[exit] pid={} code={}", current_pid, code);

    let slice = table.proc_list.make_contiguous();
    let Some(cur_idx) = find_process_index(slice, current_pid) else {
        loop {
            crate::task::executor::halt();
        }
    };

    let parent_pid = slice[cur_idx].parent_pid;
    crate::serial_println!("[exit] pid={} parent={}", current_pid, parent_pid);
    slice[cur_idx].state = ProcessState::Dead;
    slice[cur_idx].exit_code = code;
    slice[cur_idx].preempt_frame = 0;

    if let Some(parent_idx) = find_process_index(slice, parent_pid)
        && matches!(slice[parent_idx].state, ProcessState::Waiting)
    {
        crate::serial_println!("[exit] wake parent pid={}", parent_pid);
        slice[parent_idx].state = ProcessState::Running;
    }

    let next_idx = find_process_index(slice, parent_pid)
        .filter(|&idx| matches!(slice[idx].state, ProcessState::Running))
        .or_else(|| find_next_runnable_index(slice, cur_idx));

    let Some(next_idx) = next_idx else {
        crate::serial_println!("[exit] pid={} no runnable target", current_pid);
        loop {
            crate::task::executor::halt();
        }
    };

    crate::serial_println!(
        "[exit] switch dead pid={} -> pid={}",
        current_pid,
        slice[next_idx].pid,
    );
    switch_by_index(slice, cur_idx, next_idx);

    loop {
        crate::task::executor::halt();
    }
}

pub fn on_timer_tick() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
}

pub fn maybe_schedule() {
    if NEED_RESCHED.swap(false, Ordering::Relaxed) {
        schedule_now();
    }
}

pub fn schedule_now() {
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let cur_pid = id();

    // Make a contiguous slice so we can index and take raw pointers.
    let slice = table.proc_list.make_contiguous();
    if slice.len() < 2 {
        return;
    }

    let Some(cur_idx) = slice.iter().position(|p| p.pid == cur_pid) else {
        return;
    };

    let Some(next_idx) = find_next_runnable_index(slice, cur_idx) else {
        return;
    };

    switch_by_index(slice, cur_idx, next_idx);
}

#[repr(C)]
pub struct PreemptFrame {
    pub cr3: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

fn build_initial_preempt_frame(
    stack: &mut StackHelper<'_>,
    cr3: u64,
    rip: u64,
    rsp: u64,
    rax: u64,
) -> *mut PreemptFrame {
    let frame = stack.offset::<PreemptFrame>();
    *frame = PreemptFrame {
        cr3,
        r15: 0,
        r14: 0,
        r13: 0,
        r12: 0,
        r11: 0,
        r10: 0,
        r9: 0,
        r8: 0,
        rbp: 0,
        rdi: 0,
        rsi: 0,
        rdx: 0,
        rcx: 0,
        rbx: 0,
        rax,
        rip,
        cs: USER_CS.bits() as u64,
        rflags: 0x202,
        rsp,
        ss: USER_SS.bits() as u64,
    };
    frame
}

fn find_process_index(processes: &[Process], pid: u16) -> Option<usize> {
    processes.iter().position(|process| process.pid == pid)
}

fn find_next_runnable_index(processes: &[Process], current_idx: usize) -> Option<usize> {
    if processes.len() < 2 {
        return None;
    }

    for step in 1..=processes.len() {
        let idx = (current_idx + step) % processes.len();
        if idx == current_idx {
            continue;
        }
        if matches!(processes[idx].state, ProcessState::Running) {
            return Some(idx);
        }
    }

    None
}

fn find_next_preemptable_index(processes: &[Process], current_idx: usize) -> Option<usize> {
    if processes.len() < 2 {
        return None;
    }

    for step in 1..=processes.len() {
        let idx = (current_idx + step) % processes.len();
        if idx == current_idx {
            continue;
        }
        if matches!(processes[idx].state, ProcessState::Running)
            && processes[idx].preempt_frame != 0
        {
            return Some(idx);
        }
    }

    None
}

fn switch_by_index(processes: &mut [Process], cur_idx: usize, next_idx: usize) {
    if cur_idx == next_idx {
        return;
    }

    let ptr = processes.as_mut_ptr();
    unsafe {
        let cur = &mut *ptr.add(cur_idx);
        let next = &mut *ptr.add(next_idx);

        PID.store(next.pid, Ordering::SeqCst);
        switch_tasks(cur, next);
    }
}

fn restore_preempted_process(process: &mut Process) {
    io::set_fsbase()(process.fs_base);
    io::set_inactive_gsbase()(process.gs_base);

    if let Some(fpu) = process.fpu_storage.as_mut() {
        xrstor(fpu);
    }

    let kstack_top = process.kernel_gs.kernel_rsp;
    #[allow(static_mut_refs)]
    unsafe {
        crate::arch::x86_64::gdt::TSS.rsp[0] = kstack_top;
    }
    io::wrmsr(io::IA32_SYSENTER_ESP, kstack_top);
}

fn timer_preempt_common(frame: *mut PreemptFrame, from_user: u64) -> *mut PreemptFrame {
    if from_user == 0 {
        return frame;
    }

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let cur_pid = id();

    let slice = table.proc_list.make_contiguous();
    let Some(cur_idx) = find_process_index(slice, cur_pid) else {
        return frame;
    };

    {
        let cur = &mut slice[cur_idx];
        cur.preempt_frame = frame as u64;
        cur.fs_base = io::get_fsbase()();
        cur.gs_base = io::get_inactive_gsbase()(); // inactive = user GS because the ISR has swapgs'd.
        if let Some(fpu) = cur.fpu_storage.as_mut() {
            xsave(fpu);
        }
    }

    let Some(next_idx) = find_next_preemptable_index(slice, cur_idx) else {
        return frame;
    };

    let next = &mut slice[next_idx];
    PID.store(next.pid, Ordering::SeqCst);
    restore_preempted_process(next);

    next.preempt_frame as *mut PreemptFrame
}

pub extern "C" fn timer_preempt(frame: *mut PreemptFrame, from_user: u64) -> *mut PreemptFrame {
    crate::driver::timer::pit::pit_tick_isr();

    // EOI for IRQ0 (PIC timer)
    unsafe {
        crate::arch::x86_64::idt::PICS
            .lock()
            .notify_end_of_interrupt(crate::arch::x86_64::idt::PIC_1_OFFSET);
    }

    timer_preempt_common(frame, from_user)
}

pub extern "C" fn apic_timer_preempt(
    frame: *mut PreemptFrame,
    from_user: u64,
) -> *mut PreemptFrame {
    crate::driver::timer::pit::pit_tick_isr();

    // EOI for Local APIC
    crate::driver::apic::lapic::end_of_interrupt();

    timer_preempt_common(frame, from_user)
}

#[unsafe(naked)]
pub unsafe extern "C" fn iretq_init() {
    naked_asm!(
        "cli",
        // pop the error code
        "add rsp, 8",
        crate::arch::x86_64::asm_utils::pop_preserved!(),
        crate::arch::x86_64::asm_utils::pop_scratch!(),
        "iretq",
    )
}

#[repr(C)]
pub struct KernelGsData {
    kernel_rsp: u64, // offset 0
    user_rsp: u64,   // offset 8
}

pub fn init() {
    #[allow(static_mut_refs)]
    unsafe {
        PROCESS_TABLE
            .try_call_once(|| Ok::<_, ()>(ProcessTable::new()))
            .unwrap();
    }
    let (page_table_frame, _) = Cr3::read();
    let page_table = crate::memory::create_page_table(page_table_frame);
    let mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    PID.store(pid, Ordering::SeqCst);

    let proc_mm = Box::new(ProcMM::new(0));

    let switch_stack = allocate_switch_stack().unwrap().as_mut_ptr::<u8>();

    let kernel_rsp = switch_stack as u64;
    let mut stack_ptr = initial_context_stack_top(kernel_rsp);

    let mut kgs = Box::new(KernelGsData {
        kernel_rsp: 0,
        user_rsp: 0,
    });

    kgs.kernel_rsp = kernel_rsp;

    let kgs_va = VirtAddr::new(&*kgs as *const _ as u64);

    let mut stack = StackHelper::new(&mut stack_ptr);

    let task_stack = unsafe {
        let layout = Layout::from_size_align_unchecked(4096 * 16, 0x1000);
        alloc_zeroed(layout).add(layout.size())
    };

    // Skip the frame initialization - stack segment will be set elsewhere
    let kframe = stack.offset::<InterruptErrorStack>();

    // Alternatively, could store the segment selector for later use if needed
    kframe.stack.iret.ss = 0x10;
    kframe.stack.iret.cs = 0x08;
    kframe.stack.iret.rip = init_console as u64;
    kframe.stack.iret.rflags = 0x200;
    kframe.stack.iret.rsp = task_stack as u64;

    let context = stack.offset::<Context>();

    *context = Context::default();
    context.rip = iretq_init as u64;
    context.cr3 = read_cr3();

    #[allow(static_mut_refs)]
    unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .proc_list
            .push_back(Process {
                context,
                context_switch_rsp: VirtAddr::new(stack_ptr),
                fpu_storage: Some(FpuState::default()),

                pid,
                addr_size_vec: Vec::new(),
                stack: 0,
                stack_size: 0,
                entry_point: 0,
                state: ProcessState::Running,
                page_table_frame,
                mapper,
                pwd: "/".to_string(),
                fd_table: Vec::new(),
                kernel_gs: kgs,
                gs_base: kgs_va,
                fs_base: VirtAddr::zero(),
                proc_mm,
                parent_pid: 1,
                stdio_flags: [O_RDONLY, O_WRONLY, O_WRONLY],
                stdio_fd_flags: [0; 3],
                stdio_target: [-1; 3],
                exit_code: 0,
                preempt_frame: 0,
            })
    }

    let (f, _) = Cr3::read();
    let page_table = crate::memory::create_page_table(f);
    let mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

    let mut idle_task = Process {
        context: core::ptr::null_mut(),
        stack: 0,
        kernel_gs: Box::new(KernelGsData {
            kernel_rsp: 0,
            user_rsp: 0,
        }),
        fs_base: VirtAddr::zero(),
        gs_base: VirtAddr::zero(),
        proc_mm: Box::new(ProcMM::new(0)),
        parent_pid: 1,
        pid: 1,
        pwd: String::from("/"),
        context_switch_rsp: VirtAddr::zero(),
        mapper,
        fpu_storage: Some(FpuState::default()),
        entry_point: 0,
        addr_size_vec: Vec::new(),
        page_table_frame: f,
        fd_table: Vec::new(),
        state: ProcessState::Running,
        stack_size: 0,
        stdio_flags: [O_RDONLY, O_WRONLY, O_WRONLY],
        stdio_fd_flags: [0; 3],
        stdio_target: [-1; 3],
        exit_code: 0,
        preempt_frame: 0,
    };

    idle_task.gs_base = VirtAddr::new(&*idle_task.kernel_gs as *const _ as u64);

    #[allow(static_mut_refs)]
    let proc = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(1).unwrap() };

    switch_tasks(&mut idle_task, proc);
}

#[repr(C)]
#[derive(Clone)]
struct AuxvEntry {
    key: u64,
    value: u64,
}

fn build_initial_stack(
    mut rsp: u64,
    aux_entry: u64,
    at_base: u64,
    argv: Option<&[&str]>,
    envp: Option<&[&str]>,
    phdr_addr: u64, // runtime VA (load_base + e_phoff)
    phent: u64,     // 56 for Elf64_Phdr
    phnum: u64,
    execfn_ptr: Option<u64>,   // usually argv[0]
    random16_ptr: Option<u64>, // 16 bytes placed on stack
) -> u64 {
    // helper: push null-terminated bytes, return ptr
    fn push_bytes(rsp: &mut u64, bytes: &[u8]) -> u64 {
        *rsp -= (bytes.len() as u64) + 1;
        let p = *rsp as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            *p.add(bytes.len()) = 0;
        }
        *rsp
    }
    // helper: push raw bytes without a trailing NUL (for AT_RANDOM)
    fn push_raw(rsp: &mut u64, bytes: &[u8]) -> u64 {
        *rsp -= bytes.len() as u64;
        let p = *rsp as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        }
        *rsp
    }
    //
    // place strings first (argv/envp), record their pointers in-order
    let mut argv_ptrs: Vec<u64> = Vec::new();
    let mut envp_ptrs: Vec<u64> = Vec::new();

    if let Some(envs) = envp {
        for &e in envs.iter().rev() {
            envp_ptrs.push(push_bytes(&mut rsp, e.as_bytes()));
        }
        envp_ptrs.reverse();
    }
    if let Some(args) = argv {
        for &a in args.iter() {
            argv_ptrs.push(push_bytes(&mut rsp, a.as_bytes()));
        }
        argv_ptrs.reverse();
    }

    // optional: place 16 random bytes and execfn string if you haven't already
    let rand_ptr = if let Some(p) = random16_ptr {
        p
    } else {
        // simple deterministic bytes if you don't have RNG yet (acceptable to start)
        let bytes = [0u8; 16];
        push_raw(&mut rsp, &bytes)
    };
    let execfn = execfn_ptr
        .or_else(|| argv_ptrs.get(0).copied())
        .unwrap_or(0);

    let aux_vec: Vec<AuxvEntry> = vec![
        AuxvEntry {
            key: 3,
            value: phdr_addr,
        }, // AT_PHDR
        AuxvEntry {
            key: 4,
            value: phent,
        }, // AT_PHENT
        AuxvEntry {
            key: 5,
            value: phnum,
        }, // AT_PHNUM
        AuxvEntry {
            key: 7,
            value: at_base,
        }, // AT_BASE
        AuxvEntry {
            key: 6,
            value: 4096,
        }, // AT_PAGESZ
        AuxvEntry {
            key: 9,
            value: aux_entry,
        }, // AT_ENTRY
        AuxvEntry {
            key: 25,
            value: rand_ptr,
        }, // AT_RANDOM
        AuxvEntry {
            key: 31,
            value: execfn,
        }, // AT_EXECFN
        AuxvEntry { key: 17, value: 0 }, // AT_UID
        AuxvEntry { key: 18, value: 0 }, // AT_EUID
        AuxvEntry { key: 19, value: 0 }, // AT_GID
        AuxvEntry { key: 20, value: 0 }, // AT_EGID
        AuxvEntry {
            key: 23,
            value: 100,
        }, // AT_CLKTCK
        AuxvEntry { key: 0, value: 0 },  // AT_NULL
    ];

    // ---- compute padding so final %rsp follows SysV (16-byte aligned on entry) ----
    let aux_bytes = (size_of::<AuxvEntry>() * aux_vec.len()) as u64;
    let env_bytes = ((envp_ptrs.len() + 1) * size_of::<u64>()) as u64; // +NULL
    let argv_bytes = ((argv_ptrs.len() + 1) * size_of::<u64>()) as u64; // +NULL
    let total_bytes = aux_bytes + env_bytes + argv_bytes + size_of::<u64>() as u64; // +argc
    let pad = rsp.wrapping_sub(total_bytes) & 0xF; // ensure (final_rsp % 16) == 0
    if pad != 0 {
        rsp -= pad;
        unsafe { core::ptr::write_bytes(rsp as *mut u8, 0, pad as usize) };
    }

    // ---- write auxv (topmost among these tables) ----

    rsp -= aux_bytes;
    unsafe {
        core::ptr::copy_nonoverlapping(aux_vec.as_ptr(), rsp as *mut AuxvEntry, aux_vec.len());
    }

    // ---- envp NULL then envp pointers (envp[0] ends closest to argv) ----
    rsp -= 8;
    unsafe {
        *(rsp as *mut u64) = 0;
    }
    for &p in envp_ptrs.iter() {
        rsp -= 8;
        unsafe {
            *(rsp as *mut u64) = p;
        }
    }

    // ---- argv NULL then argv pointers (argv[0] ends closest to argc) ----
    rsp -= 8;
    unsafe {
        *(rsp as *mut u64) = 0;
    }
    for &p in &argv_ptrs {
        rsp -= 8;
        unsafe {
            *(rsp as *mut u64) = p;
        }
    }

    // ---- argc (alignment handled above) ----
    rsp -= 8;
    unsafe {
        *(rsp as *mut u64) = argv_ptrs.len() as u64;
    }

    rsp
}

struct LoadedImage {
    entry_point: u64,
    phdr_va: u64,
    phent: u64,
    phnum: u64,
    max_end: u64,
    load_base: u64,
    interp_path: Option<String>,
}

fn load_elf_image(
    content_buf: &[u8],
    mapper: &mut OffsetPageTable<'_>,
    addr_size_vec: &mut Vec<(u64, usize)>,
    dyn_base_hint: Option<u64>,
    capture_interp: bool,
) -> Result<LoadedImage, ()> {
    if content_buf.get(0..4) != Some(&ELF_MAGIC) {
        return Err(());
    }

    let obj = object::File::parse(content_buf).map_err(|_| ())?;
    let eh = unsafe { &*(content_buf.as_ptr() as *const Elf64Ehdr) };
    let e_phoff = eh.e_phoff;
    let e_phentsize = eh.e_phentsize as u64;
    let e_phnum = eh.e_phnum as u64;

    let mut load_bias = 0;
    if eh.e_type == 3 {
        load_bias = dyn_base_hint.unwrap_or(MAIN_DYN_LOAD_BASE);
    }

    let mut phdr_va = 0;
    let mut interp_path = None;

    for i in 0..e_phnum {
        let ph = unsafe {
            &*(content_buf
                .as_ptr()
                .add((e_phoff + i * e_phentsize) as usize) as *const Elf64Phdr)
        };
        if ph.p_type == PT_PHDR {
            phdr_va = load_bias + ph.p_vaddr;
        } else if capture_interp && ph.p_type == PT_INTERP {
            let start = ph.p_offset as usize;
            let len = ph.p_filesz as usize;
            if start + len <= content_buf.len() {
                if let Ok(s) = core::str::from_utf8(&content_buf[start..start + len]) {
                    interp_path = Some(s.trim_end_matches('\0').to_string());
                }
            }
        }
    }

    if phdr_va == 0 {
        let ph_tbl_start = e_phoff;
        let ph_tbl_end = e_phoff + e_phentsize * e_phnum;
        for i in 0..e_phnum {
            let ph = unsafe {
                &*(content_buf
                    .as_ptr()
                    .add((e_phoff + i * e_phentsize) as usize)
                    as *const Elf64Phdr)
            };
            if ph.p_type == PT_LOAD {
                let seg_start = ph.p_offset;
                let seg_end = ph.p_offset + ph.p_filesz;
                if ph_tbl_start >= seg_start && ph_tbl_end <= seg_end {
                    phdr_va = load_bias + ph.p_vaddr + (e_phoff - ph.p_offset);
                    break;
                }
            }
        }
    }

    let mut max_end = 0;
    for segment in obj.segments() {
        if let Ok(data) = segment.data() {
            let size = segment.size() as usize;
            if size == 0 {
                continue;
            }
            let base_addr = load_bias + segment.address();
            let seg_end = base_addr + size as u64;
            if seg_end > max_end {
                max_end = seg_end;
            }

            if let SegmentFlags::Elf { .. } = segment.flags() {
                if alloc_pages(mapper, base_addr, size, true, true).is_err() {
                    return Err(());
                }
                addr_size_vec.push((base_addr, size));

                let src = data.as_ptr();
                let dst = base_addr as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(src, dst, data.len());
                    if size > data.len() {
                        core::ptr::write_bytes(dst.add(data.len()), 0, size - data.len());
                    }
                }
            }
        }
    }

    Ok(LoadedImage {
        entry_point: eh.e_entry + load_bias,
        phdr_va,
        phent: e_phentsize,
        phnum: e_phnum,
        max_end,
        load_base: load_bias,
        interp_path,
    })
}

fn load_interpreter_image(
    path: &str,
    mapper: &mut OffsetPageTable<'_>,
    addr_size_vec: &mut Vec<(u64, usize)>,
) -> Result<LoadedImage, ()> {
    #[allow(static_mut_refs)]
    let interp_buf = {
        let vfs = unsafe { VFS.read() };
        let mut node = vfs.open(path).map_err(|_| ())?;
        let mut buf = vec![0u8; node.metadata.size];
        node.read(0, &mut buf).map_err(|_| ())?;

        buf
    };

    load_elf_image(
        interp_buf.as_slice(),
        mapper,
        addr_size_vec,
        Some(INTERP_DYN_LOAD_BASE),
        false,
    )
}

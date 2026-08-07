pub mod mem;
pub mod switch;
mod task;
pub(crate) mod user;

use crate::arch::x86_64::gdt::{SegmentSelector, USER_CS, USER_SS};
use crate::arch::x86_64::io;
use crate::arch::x86_64::io::{wrmsr, IA32_FS_BASE, IA32_GS_BASE};
use crate::driver::disk::dummy_blockdev;
use crate::kernel_utils::exec::jump_to_user;
use crate::println;
use crate::sys::console::init_console;
use crate::sys::console::tty::TtyDev;
use crate::sys::fs::memfd::MemFd;
use crate::sys::fs::pipe::PipeEnd;
use crate::sys::fs::vfs::{Metadata, VfsNode, VfsNodeOps, VFS};
use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::memory::{
    alloc_pages_unflushed, allocate_zeroed_frame, deallocate_frame, kernel_page_table,
    map_user_frame, phys_mem_offset, phys_to_virt, user_page_flags, user_page_flags_with_access,
};
use crate::sys::proc::mem::{ElfRegion, ProcMM, VmPermissions};
use crate::sys::proc::switch::read_cr3;
use crate::sys::proc::task::{allocate_switch_stack, switch_tasks, Context, FpuState};
use crate::sys::proc::user::USER_ENV;
use crate::utils::{sync::WaitQueue, StackHelper};
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
use core::sync::atomic::{AtomicU16, Ordering};
use crate::utils::sync::Mutex;
use crate::utils::sync::RwLock;
use spin::Once;
use twilight_common::syscall::types::{O_RDONLY, O_WRONLY};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::{MappedFrame, TranslateResult};
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, Page, PhysFrame, Size4KiB, Translate,
};
use x86_64::VirtAddr;

pub static mut PROCESS_TABLE: Once<ProcessTable> = Once::new();
static POLL_WAIT_QUEUE: WaitQueue = WaitQueue::new();

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
pub const USER_STACK_SIZE: usize = 0x64000;
const MAIN_DYN_LOAD_BASE: u64 = 0x4000_0000;
const INTERP_DYN_LOAD_BASE: u64 = 0x6000_0000;
const PAGE_SIZE_U64: u64 = 4096;
const STATIC_TLS_SPILL_PAGES: u64 = 2;
const TASK_COMM_LEN: usize = 16;
static NEXT_PID: AtomicU16 = AtomicU16::new(1);
static PID: AtomicU16 = AtomicU16::new(0);
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
#[derive(Debug, Clone, Copy)]
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
#[derive(Debug, Clone, Copy)]
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
const PT_TLS: u32 = 7;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

// ================== Process ==================

#[repr(C)]
#[derive(Debug)]
pub enum ProcessState {
    /// Currently executing on the BSP.
    Running,
    /// Eligible for scheduler selection but not currently executing.
    Runnable,
    Sleeping,
    Waiting,
    SignalWait,
    AwaitingIo,
    Stopped,
    Dead,
}

pub const SIGNAL_COUNT: usize = 65;
pub const SIGCHLD: usize = 17;
pub const SIGKILL: usize = 9;
pub const SIGPIPE: usize = 13;
pub const SIGSTOP: usize = 19;
pub const SIGBUS: usize = 7;
pub const SIGSEGV: usize = 11;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SignalAction {
    pub handler: u64,
    pub mask: [u64; 2],
    pub flags: u64,
    pub restorer: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SignalAltStack {
    pub sp: u64,
    pub flags: i32,
    pub size: u64,
}

impl Default for SignalAltStack {
    fn default() -> Self {
        Self {
            sp: 0,
            flags: 2,
            size: 0,
        }
    }
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
            crate::serial_println!(
                "[proc::run] switch parent pid={} -> child pid={}",
                prev_task.pid,
                next_task.pid,
            );
            if !switch_tasks_with_new_scheduler_guard(prev_task, next_task) {
                prev_task.state = ProcessState::Running;
                PID.store(current_pid, Ordering::SeqCst);
                crate::serial_println!("[sched] process launch switch deferred");
                return;
            }
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
    Pipe(Arc<PipeEnd>),
    Socket(crate::sys::net::socket::SocketFile),
    MemFd(Arc<Mutex<MemFd>>),
}

#[derive(Clone)]
pub struct FdEntry {
    pub file: Arc<Mutex<OpenFile>>,
    pub fd_flags: i32,
}

fn standard_fd_table() -> Vec<Option<FdEntry>> {
    [O_RDONLY, O_WRONLY, O_WRONLY]
        .into_iter()
        .map(|status_flags| {
            let tty_ops: Arc<RwLock<dyn VfsNodeOps>> = Arc::new(RwLock::new(TtyDev));
            let tty_node = Arc::new(Mutex::new(VfsNode::new(
                dummy_blockdev(),
                Metadata::chr(4, "tty"),
                tty_ops,
            )));
            Some(FdEntry {
                file: Arc::new(Mutex::new(OpenFile {
                    kind: OpenFileKind::Vfs(tty_node),
                    seek: 0,
                    path: "/dev/tty".to_string(),
                    status_flags,
                })),
                fd_flags: 0,
            })
        })
        .collect()
}

fn task_comm_from_path(path: &str) -> [u8; TASK_COMM_LEN] {
    let mut comm = [0; TASK_COMM_LEN];
    let name = path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path);
    let len = name.len().min(TASK_COMM_LEN - 1);
    comm[..len].copy_from_slice(&name.as_bytes()[..len]);
    comm
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
    pub tgid: u16,
    pub is_thread: bool,
    pub parent_pid: u16,
    pub pgid: u16,
    pub sid: u16,
    pub state: ProcessState,
    pub addr_size_vec: Vec<(u64, usize)>,
    pub exe_path: String,
    comm: [u8; TASK_COMM_LEN],
    pub pwd: String,
    pub fd_table: Vec<Option<FdEntry>>,
    pub umask: u16,
    pub proc_mm: Arc<Mutex<ProcMM>>,
    pub exit_code: i32,
    pub wait_status: i32,
    pub wait_reported: bool,
    pub preempt_frame: u64, // saved RSP to PreemptFrame on this task's kernel stack
    pub pending_io: bool,
    pub signal_actions: [SignalAction; SIGNAL_COUNT],
    pub signal_mask: [u64; 2],
    pub pending_signals: u64,
    pub sigsuspend_saved_mask: [u64; 2],
    pub in_sigsuspend: bool,
    pub signal_alt_stack: SignalAltStack,
    /// Outstanding wait token, deadline, and resume reason for deadline-ordered
    /// blocking (#66). `wait_token` is `Some` iff this process is currently
    /// blocked in `sys::timer::block_current_until` (or an I/O timeout).
    pub wait_token: Option<crate::sys::timer::WaitToken>,
    pub wait_deadline_ns: Option<u64>,
    pub wake_reason: Option<crate::sys::timer::WakeReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageFaultResolution {
    Resolved,
    Invalid,
    BusError,
}

impl Process {
    pub fn set_comm(&mut self, name: &[u8]) {
        self.comm.fill(0);
        let len = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len())
            .min(TASK_COMM_LEN - 1);
        self.comm[..len].copy_from_slice(&name[..len]);
    }

    pub fn set_comm_from_path(&mut self, path: &str) {
        let name = path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(path);
        self.set_comm(name.as_bytes());
    }

    pub fn comm(&self) -> [u8; TASK_COMM_LEN] {
        self.comm
    }

    pub fn fd_entry(&self, fd: i32) -> Option<&FdEntry> {
        usize::try_from(fd)
            .ok()
            .and_then(|index| self.fd_table.get(index))
            .and_then(Option::as_ref)
    }

    pub fn fd_entry_mut(&mut self, fd: i32) -> Option<&mut FdEntry> {
        usize::try_from(fd)
            .ok()
            .and_then(|index| self.fd_table.get_mut(index))
            .and_then(Option::as_mut)
    }

    pub fn install_fd(&mut self, entry: FdEntry, min_fd: i32) -> Result<i32, i32> {
        let start = usize::try_from(min_fd).map_err(|_| twilight_common::syscall::types::EINVAL)?;
        if self.fd_table.len() < start {
            self.fd_table.resize_with(start, || None);
        }

        if let Some(index) = self.fd_table[start..]
            .iter()
            .position(Option::is_none)
            .map(|offset| start + offset)
        {
            self.fd_table[index] = Some(entry);
            return i32::try_from(index).map_err(|_| twilight_common::syscall::types::EMFILE);
        }

        let index = self.fd_table.len();
        self.fd_table.push(Some(entry));
        i32::try_from(index).map_err(|_| twilight_common::syscall::types::EMFILE)
    }

    pub fn replace_fd(&mut self, fd: i32, entry: FdEntry) -> Result<Option<FdEntry>, i32> {
        let index = usize::try_from(fd).map_err(|_| twilight_common::syscall::types::EBADF)?;
        if self.fd_table.len() <= index {
            self.fd_table.resize_with(index + 1, || None);
        }
        Ok(self.fd_table[index].replace(entry))
    }

    pub fn close_fd(&mut self, fd: i32) -> Result<FdEntry, i32> {
        let index = usize::try_from(fd).map_err(|_| twilight_common::syscall::types::EBADF)?;
        self.fd_table
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(twilight_common::syscall::types::EBADF)
    }

    pub fn close_all_fds(&mut self) {
        self.fd_table.clear();
    }

    pub fn queue_signal(&mut self, sig: usize) {
        if !(1..=64).contains(&sig) {
            return;
        }
        self.pending_signals |= signal_bit(sig);
        if matches!(
            self.state,
            ProcessState::Waiting | ProcessState::SignalWait | ProcessState::AwaitingIo
        ) {
            self.state = ProcessState::Runnable;
            self.pending_io = false;
        }
    }

    pub fn has_unblocked_signal(&self) -> bool {
        self.next_unblocked_signal().is_some()
    }

    pub fn next_unblocked_signal(&self) -> Option<usize> {
        let unblocked = self.pending_signals & !self.signal_mask[0];
        if unblocked == 0 {
            None
        } else {
            Some(unblocked.trailing_zeros() as usize + 1)
        }
    }

    pub fn dequeue_signal(&mut self, sig: usize) {
        let _pt_guard = crate::sys::preempt::enter_process_table_context();
        if (1..=64).contains(&sig) {
            self.pending_signals &= !signal_bit(sig);
        }
    }

    pub fn resolve_page_fault(
        &mut self,
        fault_addr: u64,
        write: bool,
        execute: bool,
    ) -> PageFaultResolution {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(fault_addr));
        if self.mapper.translate_page(page).is_ok() {
            return PageFaultResolution::Resolved;
        }

        let page_base = page.start_address().as_u64() as usize;
        let Some(mut plan) = self.proc_mm.lock().page_fault_plan(page_base) else {
            return PageFaultResolution::Invalid;
        };
        if !plan.permissions.allows(write, execute) {
            return PageFaultResolution::Invalid;
        }

        let Some(frame) = allocate_zeroed_frame() else {
            return PageFaultResolution::BusError;
        };
        let frame_ptr = phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();

        for fragment in &mut plan.fragments {
            // SAFETY: `frame` is exclusively owned here, and every fragment
            // was bounded to this single 4 KiB page by `page_fault_plan`.
            let destination = unsafe {
                core::slice::from_raw_parts_mut(frame_ptr.add(fragment.page_offset), fragment.len)
            };
            if fragment
                .file
                .read_exact(fragment.file_offset, destination)
                .is_err()
            {
                deallocate_frame(frame);
                return PageFaultResolution::BusError;
            }
        }

        if self.mapper.translate_page(page).is_ok() {
            deallocate_frame(frame);
            return PageFaultResolution::Resolved;
        }

        let flags = user_page_flags(plan.permissions.write, plan.permissions.execute);
        if map_user_frame(&mut self.mapper, page_base as u64, frame, flags).is_err() {
            deallocate_frame(frame);
            return PageFaultResolution::BusError;
        }
        PageFaultResolution::Resolved
    }

    pub fn new(
        mut executable: VfsNode,
        exe_path: &str,
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
        let mut elf_regions = Vec::new();

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
        let needs_static_tls_spill: bool;

        if executable.metadata.size >= ELF_MAGIC.len() {
            match load_elf_image(
                &mut executable,
                &mut elf_regions,
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
                    needs_static_tls_spill = main_img.has_tls && main_img.interp_path.is_none();

                    if let Some(interp_path) = main_img.interp_path {
                        match load_interpreter_image(interp_path.as_str(), &mut elf_regions) {
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
                    if let Ok(_) = alloc_pages_unflushed(
                        &mut mapper,
                        user_stack_base,
                        USER_STACK_SIZE,
                        true,
                        false,
                    ) {
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

        if needs_static_tls_spill {
            max_end = reserve_static_tls_spill(&mut mapper, &mut addr_size_vec, max_end)?;
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
        let mut memory = ProcMM::new(max_end as usize);
        memory.elf_regions = elf_regions;
        let proc_mm = Arc::new(Mutex::new(memory));
        let sid = if parent_pid == 0 {
            pid
        } else {
            process_session_id(parent_pid).unwrap_or(pid)
        };

        let switch_stack = allocate_switch_stack().unwrap().as_mut_ptr::<u8>();

        let stack_ptr = switch_stack as u64;

        let mut kgs = Box::new(KernelGsData {
            kernel_rsp: 0, // The top of the stack for syscall/interrupt entry
            user_rsp: 0,
        });

        kgs.kernel_rsp = stack_ptr;

        let p = Process {
            context: core::ptr::null_mut(), // Point to the constructed context
            context_switch_rsp: VirtAddr::new(stack_ptr), // This field might be redundant if we use context, but keep it consistent
            fpu_storage: Some(FpuState::default()),

            stack: user_rsp,
            stack_size: USER_STACK_SIZE,
            entry_point: entry_point_addr,
            pid,
            tgid: pid,
            is_thread: false,
            mapper,
            page_table_frame,
            state: ProcessState::Runnable,
            addr_size_vec,
            exe_path: exe_path.to_string(),
            comm: task_comm_from_path(exe_path),
            pwd: pwd.to_string(),
            kernel_gs: kgs,
            fs_base: VirtAddr::zero(),
            gs_base: VirtAddr::zero(),
            fd_table: standard_fd_table(),
            proc_mm,
            parent_pid,
            pgid: pid,
            sid,
            umask: 0o022,
            exit_code: 0,
            wait_status: 0,
            wait_reported: false,
            preempt_frame: 0,
            pending_io: false,
            signal_actions: [SignalAction::default(); SIGNAL_COUNT],
            signal_mask: [0; 2],
            pending_signals: 0,
            sigsuspend_saved_mask: [0; 2],
            in_sigsuspend: false,
            signal_alt_stack: SignalAltStack::default(),
            wait_token: None,
            wait_deadline_ns: None,
            wake_reason: None,
        };
        Ok(p)
    }

    pub fn exec(
        &mut self,
        executable: &mut VfsNode,
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
        let mut elf_regions = Vec::new();
        let user_stack_top = VirtAddr::new(USER_STACK_TOP);

        let mut entry_point_addr: u64;
        let aux_entry_point: u64;
        let mut at_base: u64 = 0;
        let phdr_va: u64;
        let phent: u64;
        let phnum: u64;
        let mut max_end: u64;
        let needs_static_tls_spill: bool;

        if executable.metadata.size >= ELF_MAGIC.len() {
            match load_elf_image(executable, &mut elf_regions, Some(MAIN_DYN_LOAD_BASE), true) {
                Ok(main_img) => {
                    entry_point_addr = main_img.entry_point;
                    aux_entry_point = entry_point_addr;
                    phdr_va = main_img.phdr_va;
                    phent = main_img.phent;
                    phnum = main_img.phnum;
                    max_end = main_img.max_end;
                    needs_static_tls_spill = main_img.has_tls && main_img.interp_path.is_none();

                    if let Some(interp_path) = main_img.interp_path {
                        // We must read the interpreter file.
                        // We can't use VFS normally if it relies on current process state?
                        // VFS uses `Process::current()`? No, it usually just takes paths.
                        // But accessing "user pointers" in `exec` is tricky if we just switched CR3.
                        // However, `load_interpreter_image` takes a path string (kernel memory), not user pointer.
                        // We should be fine.
                        match load_interpreter_image(interp_path.as_str(), &mut elf_regions) {
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
                    if alloc_pages_unflushed(
                        &mut mapper,
                        user_stack_base,
                        USER_STACK_SIZE,
                        true,
                        false,
                    )
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

        if needs_static_tls_spill {
            max_end = reserve_static_tls_spill(&mut mapper, &mut addr_size_vec, max_end)?;
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

        let mut memory = ProcMM::new(max_end as usize);
        memory.elf_regions = elf_regions;
        let proc_mm = Arc::new(Mutex::new(memory));

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

        // Reset FS/GS base – the old TLS pointer from the parent image is
        // invalid after exec.  musl will set up a new one via arch_prctl.
        self.fs_base = VirtAddr::zero();
        self.gs_base = VirtAddr::zero();

        // Reset signal dispositions (SIG_DFL) after exec, per POSIX.
        self.signal_actions = [SignalAction::default(); SIGNAL_COUNT];
        self.signal_mask = [0; 2];
        self.pending_signals = 0;

        // Preserve descriptors across exec except those marked close-on-exec.
        for slot in &mut self.fd_table {
            if slot.as_ref().is_some_and(|entry| entry.fd_flags & 1 != 0) {
                *slot = None;
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
        let proc_mm = self.proc_mm.lock();
        // crate::serial_println!(
        //     "[fork] parent={} child={} rip={:#x} rsp={:#x} regions={} heap={:#x}-{:#x} mmap={}",
        //     self.pid,
        //     pid,
        //     tf.iret.rip,
        //     tf.iret.rsp,
        //     self.addr_size_vec.len(),
        //     proc_mm.heap_start,
        //     proc_mm.mapped_heap_end,
        //     proc_mm.mmap_regions.len(),
        // );

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

        // 2. Copy only pages that are resident in the parent. Lazy anonymous
        // and ELF pages remain non-present in the child.
        let mut regions_to_copy = self.addr_size_vec.clone();
        if proc_mm.mapped_heap_end > proc_mm.heap_start {
            regions_to_copy.push((
                proc_mm.heap_start as u64,
                proc_mm.mapped_heap_end - proc_mm.heap_start,
            ));
        }
        for region in &proc_mm.mmap_regions {
            regions_to_copy.push((region.base as u64, region.len));
        }
        for region in &proc_mm.elf_regions {
            regions_to_copy.push((region.base as u64, region.len));
        }

        let mut pages_to_copy = Vec::new();
        for (addr, size) in &regions_to_copy {
            if *size == 0 {
                continue;
            }
            let start = Page::<Size4KiB>::containing_address(VirtAddr::new(*addr));
            let end = Page::<Size4KiB>::containing_address(VirtAddr::new(
                addr.saturating_add(*size as u64).saturating_sub(1),
            ));
            for page in Page::range_inclusive(start, end) {
                pages_to_copy.push(page.start_address().as_u64());
            }
        }
        pages_to_copy.sort_unstable();
        pages_to_copy.dedup();

        let child_proc_mm = Arc::new(Mutex::new(proc_mm.clone()));
        let shared_pages = pages_to_copy
            .iter()
            .map(|addr| (*addr, proc_mm.is_shared_page(*addr as usize)))
            .collect::<Vec<_>>();
        drop(proc_mm);

        for (addr, shared) in shared_pages {
            let TranslateResult::Mapped {
                frame: MappedFrame::Size4KiB(parent_frame),
                flags,
                ..
            } = self.mapper.translate(VirtAddr::new(addr))
            else {
                continue;
            };
            let child_flags = user_page_flags_with_access(
                flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE),
                flags.contains(x86_64::structures::paging::PageTableFlags::WRITABLE),
                !flags.contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE),
            );

            if shared {
                if map_user_frame(&mut mapper, addr, parent_frame, child_flags).is_err() {
                    return Err(());
                }
                continue;
            }

            let Some(child_frame) = allocate_zeroed_frame() else {
                return Err(());
            };
            let source = phys_to_virt(parent_frame.start_address()).as_ptr::<u8>();
            let destination = phys_to_virt(child_frame.start_address()).as_mut_ptr::<u8>();
            // SAFETY: both frames are valid 4 KiB physical-memory mappings;
            // the child frame is exclusively owned and does not overlap the parent.
            unsafe {
                core::ptr::copy_nonoverlapping(source, destination, PAGE_SIZE_U64 as usize);
            }
            if map_user_frame(&mut mapper, addr, child_frame, child_flags).is_err() {
                deallocate_frame(child_frame);
                return Err(());
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
            tgid: pid,
            is_thread: false,
            mapper,
            page_table_frame,
            state: ProcessState::Runnable,
            addr_size_vec: self.addr_size_vec.clone(),
            exe_path: self.exe_path.clone(),
            comm: self.comm,
            pwd: self.pwd.clone(),
            fd_table: new_fd_table,
            kernel_gs: kgs,
            gs_base: self.gs_base,
            fs_base: live_fs_base,
            proc_mm: child_proc_mm,
            parent_pid: self.pid,
            pgid: self.pgid,
            sid: self.sid,
            umask: self.umask,
            exit_code: 0,
            wait_status: 0,
            wait_reported: false,
            preempt_frame: 0,
            pending_io: false,
            signal_actions: self.signal_actions,
            signal_mask: self.signal_mask,
            pending_signals: 0,
            sigsuspend_saved_mask: [0; 2],
            in_sigsuspend: false,
            signal_alt_stack: self.signal_alt_stack,
            wait_token: None,
            wait_deadline_ns: None,
            wake_reason: None,
        };

        // crate::serial_println!("[fork] child={} ready", pid);
        Ok(child)
    }

    pub fn clone_thread(
        &mut self,
        tf: &InterruptStack,
        child_stack: u64,
        tls: u64,
    ) -> Result<Process, ()> {
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let (_, flags) = Cr3::read();
        let page_table = crate::sys::memory::create_page_table(self.page_table_frame);
        let mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        let switch_stack = allocate_switch_stack().map_err(|_| ())?.as_mut_ptr::<u8>();
        let kernel_rsp = switch_stack as u64;
        let mut stack_ptr = initial_context_stack_top(kernel_rsp);
        let kgs = Box::new(KernelGsData {
            kernel_rsp,
            user_rsp: 0,
        });
        let mut stack = StackHelper::new(&mut stack_ptr);
        let kframe = stack.offset::<InterruptErrorStack>();
        *kframe = InterruptErrorStack {
            code: 0,
            stack: *tf,
        };
        kframe.stack.scratch.rax = 0;
        if child_stack != 0 {
            kframe.stack.iret.rsp = child_stack;
        }

        let context = stack.offset::<Context>();
        *context = Context::default();
        context.rip = iretq_init as u64;
        context.cr3 = self.page_table_frame.start_address().as_u64() | flags.bits();

        Ok(Process {
            context,
            context_switch_rsp: VirtAddr::new(stack_ptr),
            fpu_storage: self.fpu_storage,
            kernel_gs: kgs,
            gs_base: self.gs_base,
            fs_base: VirtAddr::new(tls),
            stack: child_stack,
            stack_size: 0,
            mapper,
            entry_point: self.entry_point,
            page_table_frame: self.page_table_frame,
            pid,
            tgid: self.tgid,
            is_thread: true,
            parent_pid: self.parent_pid,
            pgid: self.pgid,
            sid: self.sid,
            state: ProcessState::Runnable,
            addr_size_vec: Vec::new(),
            exe_path: self.exe_path.clone(),
            comm: self.comm,
            pwd: self.pwd.clone(),
            fd_table: self.fd_table.clone(),
            umask: self.umask,
            proc_mm: self.proc_mm.clone(),
            exit_code: 0,
            wait_status: 0,
            wait_reported: false,
            preempt_frame: 0,
            pending_io: false,
            signal_actions: self.signal_actions,
            signal_mask: self.signal_mask,
            pending_signals: 0,
            sigsuspend_saved_mask: [0; 2],
            in_sigsuspend: false,
            signal_alt_stack: SignalAltStack::default(),
            wait_token: None,
            wait_deadline_ns: None,
            wake_reason: None,
        })
    }

    pub fn cleanup(&mut self, table_frame: PhysFrame) {
        let mut owned_ranges = self.addr_size_vec.clone();
        let mut shared_ranges = Vec::new();
        {
            let proc_mm = self.proc_mm.lock();
            if proc_mm.mapped_heap_end > proc_mm.heap_start {
                owned_ranges.push((
                    proc_mm.heap_start as u64,
                    proc_mm.mapped_heap_end - proc_mm.heap_start,
                ));
            }
            for region in &proc_mm.mmap_regions {
                let range = (region.base as u64, region.len);
                if region.kind == crate::sys::proc::mem::MmapKind::Shared {
                    shared_ranges.push(range);
                } else {
                    owned_ranges.push(range);
                }
            }
            for region in &proc_mm.elf_regions {
                owned_ranges.push((region.base as u64, region.len));
            }
        }

        for (addr, size) in owned_ranges {
            let _ = crate::sys::memory::dealloc_pages(&mut self.mapper, addr, size);
        }
        for (addr, size) in shared_ranges {
            let _ = crate::sys::memory::unmap_user_pages(&mut self.mapper, addr, size);
        }
        self.addr_size_vec.clear();

        with_frame_allocator(|allocator| unsafe {
            allocator.deallocate_frame(table_frame);
        });
    }
}

pub fn id() -> u16 {
    PID.load(Ordering::SeqCst)
}

pub fn resolve_current_page_fault(
    fault_addr: u64,
    write: bool,
    execute: bool,
) -> PageFaultResolution {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    let current = id();
    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { PROCESS_TABLE.get_mut() }) else {
        return PageFaultResolution::Invalid;
    };
    let Some(process) = table.get_process(current) else {
        return PageFaultResolution::Invalid;
    };
    process.resolve_page_fault(fault_addr, write, execute)
}

pub fn process_group_id(pid: u16) -> Option<u16> {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut()? };
    table
        .proc_list
        .iter()
        .find(|p| p.pid == pid)
        .map(|p| p.pgid)
}

pub fn process_session_id(pid: u16) -> Option<u16> {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut()? };
    table.proc_list.iter().find(|p| p.pid == pid).map(|p| p.sid)
}

pub fn current_process_group_id() -> u16 {
    let current = id();
    process_group_id(current).unwrap_or(current)
}

#[inline]
pub fn signal_bit(sig: usize) -> u64 {
    1u64 << (sig - 1)
}

pub fn queue_signal(pid: u16, sig: usize) -> bool {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { PROCESS_TABLE.get_mut() }) else {
        return false;
    };
    let Some(process) = table.get_process(pid) else {
        return false;
    };
    process.queue_signal(sig);
    wake_process(pid);
    true
}

pub fn current_has_unblocked_signal() -> bool {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    let current = id();
    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { PROCESS_TABLE.get_mut() }) else {
        return false;
    };
    table
        .get_process(current)
        .is_some_and(|process| process.has_unblocked_signal())
}

pub fn poll_wait_queue() -> &'static WaitQueue {
    &POLL_WAIT_QUEUE
}

fn initial_context_stack_top(kernel_rsp: u64) -> u64 {
    kernel_rsp - INITIAL_CONTEXT_STACK_GUARD
}

pub fn exit(code: i32) {
    let Some(scheduler_guard) = crate::sys::preempt::SchedulerGuard::try_enter() else {
        crate::serial_println!("[sched] exit could not enter scheduler");
        loop {
            crate::task::executor::halt();
        }
    };
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

    if slice[cur_idx].is_thread {
        slice[cur_idx].close_all_fds();
        invalidate_wait_token(&mut slice[cur_idx]);
        slice[cur_idx].state = ProcessState::Dead;
        slice[cur_idx].exit_code = code;

        if let Some(next_idx) = find_next_runnable_index(slice, cur_idx) {
            switch_by_index_guarded(slice, cur_idx, next_idx, scheduler_guard);
            loop {
                crate::task::executor::halt();
            }
        }
        // No runnable alternative yet: another task may be Sleeping and will be
        // woken by a future timer IRQ. Drop the guard so schedule_now() can run.
        drop(scheduler_guard);
        idle_until_runnable();
    }

    let parent_pid = slice[cur_idx].parent_pid;
    reparent_children(slice, current_pid);
    crate::serial_println!("[exit] pid={} parent={}", current_pid, parent_pid);
    slice[cur_idx].close_all_fds();
    invalidate_wait_token(&mut slice[cur_idx]);
    slice[cur_idx].state = ProcessState::Dead;
    slice[cur_idx].exit_code = code;
    slice[cur_idx].wait_status = (code & 0xff) << 8;
    slice[cur_idx].wait_reported = false;

    if let Some(parent_idx) = find_process_index(slice, parent_pid) {
        slice[parent_idx].queue_signal(SIGCHLD);
        if matches!(
            slice[parent_idx].state,
            ProcessState::Waiting | ProcessState::SignalWait | ProcessState::AwaitingIo
        ) {
            crate::serial_println!("[exit] wake parent pid={}", parent_pid);
            slice[parent_idx].state = ProcessState::Runnable;
            slice[parent_idx].pending_io = false;
        }
    }

    let next_idx = find_process_index(slice, parent_pid)
        .filter(|&idx| matches!(slice[idx].state, ProcessState::Runnable))
        .or_else(|| find_next_runnable_index(slice, cur_idx));

    let Some(next_idx) = next_idx else {
        // No runnable alternative yet: another task may be Sleeping and will be
        // woken by a future timer IRQ. Drop the guard so schedule_now() can run.
        drop(scheduler_guard);
        crate::serial_println!("[exit] pid={} no runnable target", current_pid);
        idle_until_runnable();
    };

    crate::serial_println!(
        "[exit] switch dead pid={} -> pid={}",
        current_pid,
        slice[next_idx].pid,
    );
    switch_by_index_guarded(slice, cur_idx, next_idx, scheduler_guard);

    loop {
        crate::task::executor::halt();
    }
}

pub fn exit_group(code: i32) -> ! {
    let Some(scheduler_guard) = crate::sys::preempt::SchedulerGuard::try_enter() else {
        crate::serial_println!("[sched] exit_group could not enter scheduler");
        loop {
            crate::task::executor::halt();
        }
    };
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let current_pid = id();
    let slice = table.proc_list.make_contiguous();
    let Some(cur_idx) = find_process_index(slice, current_pid) else {
        loop {
            crate::task::executor::halt();
        }
    };

    let tgid = slice[cur_idx].tgid;
    let Some(leader_idx) = slice.iter().position(|process| process.pid == tgid) else {
        loop {
            crate::task::executor::halt();
        }
    };
    let parent_pid = slice[leader_idx].parent_pid;
    reparent_children(slice, tgid);

    for process in slice.iter_mut().filter(|process| process.tgid == tgid) {
        process.close_all_fds();
        invalidate_wait_token(process);
        process.state = ProcessState::Dead;
        process.exit_code = code;
    }
    slice[leader_idx].wait_status = (code & 0xff) << 8;
    slice[leader_idx].wait_reported = false;

    if let Some(parent_idx) = find_process_index(slice, parent_pid) {
        slice[parent_idx].queue_signal(SIGCHLD);
    }

    let next_idx = find_process_index(slice, parent_pid)
        .filter(|&idx| matches!(slice[idx].state, ProcessState::Runnable))
        .or_else(|| find_next_runnable_index(slice, cur_idx));
    let Some(next_idx) = next_idx else {
        // No runnable alternative yet: another task may be Sleeping and will be
        // woken by a future timer IRQ. Drop the guard so schedule_now() can run.
        drop(scheduler_guard);
        idle_until_runnable();
    };

    switch_by_index_guarded(slice, cur_idx, next_idx, scheduler_guard);
    loop {
        crate::task::executor::halt();
    }
}

pub fn terminate_current_by_signal(sig: i32) -> ! {
    let Some(scheduler_guard) = crate::sys::preempt::SchedulerGuard::try_enter() else {
        crate::serial_println!("[sched] signal exit could not enter scheduler");
        loop {
            crate::task::executor::halt();
        }
    };
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };

    let current_pid = id();
    crate::serial_println!("[signal-exit] pid={} sig={}", current_pid, sig);

    let slice = table.proc_list.make_contiguous();
    let Some(cur_idx) = find_process_index(slice, current_pid) else {
        loop {
            crate::task::executor::halt();
        }
    };

    let parent_pid = slice[cur_idx].parent_pid;
    reparent_children(slice, current_pid);
    slice[cur_idx].close_all_fds();
    invalidate_wait_token(&mut slice[cur_idx]);
    slice[cur_idx].state = ProcessState::Dead;
    slice[cur_idx].exit_code = 128 + sig;
    slice[cur_idx].wait_status = sig & 0x7f;
    slice[cur_idx].wait_reported = false;

    if let Some(parent_idx) = find_process_index(slice, parent_pid) {
        slice[parent_idx].queue_signal(SIGCHLD);
        if matches!(
            slice[parent_idx].state,
            ProcessState::Waiting | ProcessState::SignalWait | ProcessState::AwaitingIo
        ) {
            slice[parent_idx].state = ProcessState::Runnable;
            slice[parent_idx].pending_io = false;
        }
    }

    let next_idx = find_process_index(slice, parent_pid)
        .filter(|&idx| matches!(slice[idx].state, ProcessState::Runnable))
        .or_else(|| find_next_runnable_index(slice, cur_idx));

    let Some(next_idx) = next_idx else {
        // No runnable alternative yet: another task may be Sleeping and will be
        // woken by a future timer IRQ. Drop the guard so schedule_now() can run.
        drop(scheduler_guard);
        crate::serial_println!("[signal-exit] pid={} no runnable target", current_pid);
        idle_until_runnable();
    };

    crate::serial_println!(
        "[signal-exit] switch dead pid={} -> pid={}",
        current_pid,
        slice[next_idx].pid,
    );
    switch_by_index_guarded(slice, cur_idx, next_idx, scheduler_guard);

    loop {
        crate::task::executor::halt();
    }
}

pub fn on_timer_tick() {
    crate::sys::preempt::set_need_resched();
    POLL_WAIT_QUEUE.notify_all();
}

pub fn maybe_schedule() {
    crate::sys::preempt::cond_resched();
}

pub fn schedule_now() -> bool {
    // Diagnostic warnings for unsafe scheduling contexts. Does not block;
    // SchedulerGuard below handles the hard checks (in_scheduler, preempt_count).
    crate::sys::preempt::warn_if_schedule_unsafe();

    let Some(scheduler_guard) = crate::sys::preempt::SchedulerGuard::try_enter() else {
        return false;
    };

    // This invocation is now responsible for the pending request, even when
    // there is currently no alternate runnable process.
    crate::sys::preempt::clear_need_resched();

    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { PROCESS_TABLE.get_mut() }) else {
        return false;
    };
    let cur_pid = id();

    // Make a contiguous slice so we can index and take raw pointers.
    let slice = table.proc_list.make_contiguous();
    if slice.len() < 2 {
        return false;
    }

    let Some(cur_idx) = slice.iter().position(|p| p.pid == cur_pid) else {
        return false;
    };

    let Some(next_idx) = find_next_runnable_index(slice, cur_idx) else {
        return false;
    };

    switch_by_index_guarded(slice, cur_idx, next_idx, scheduler_guard)
}

pub fn await_io() {
    let cur_pid = id();

    loop {
        let Some(scheduler_guard) = crate::sys::preempt::SchedulerGuard::try_enter() else {
            return;
        };
        #[allow(static_mut_refs)]
        let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
        let slice = table.proc_list.make_contiguous();
        let Some(cur_idx) = find_process_index(slice, cur_pid) else {
            return;
        };

        // A wake may have raced ahead of us since the last iteration: an IRQ
        // handler can run between dropping the guard (or returning from a
        // context switch) and re-acquiring it here, because SchedulerGuard
        // disables preemption but *not* interrupts. If `wake_process` already
        // flipped us to Runnable or set pending_io, honour that wake instead of
        // clobbering it back to AwaitingIo — otherwise the wakeup is lost and
        // this task blocks forever.
        if slice[cur_idx].pending_io {
            slice[cur_idx].pending_io = false;
            if matches!(slice[cur_idx].state, ProcessState::Runnable | ProcessState::AwaitingIo) {
                slice[cur_idx].state = ProcessState::Running;
            }
            return;
        }
        if matches!(slice[cur_idx].state, ProcessState::Runnable) {
            slice[cur_idx].state = ProcessState::Running;
            return;
        }

        slice[cur_idx].state = ProcessState::AwaitingIo;

        if let Some(next_idx) = find_next_runnable_index(slice, cur_idx) {
            switch_by_index_guarded(slice, cur_idx, next_idx, scheduler_guard);

            // Context switched back. Check if we were woken properly.
            let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
            #[allow(static_mut_refs)]
            let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
            if let Some(process) = table.get_process(cur_pid) {
                if process.pending_io {
                    process.pending_io = false;
                    return;
                }
                if matches!(process.state, ProcessState::Running) {
                    return; // Woken by wake_process explicitly setting state
                }
            }
        } else {
            // No other runnable process. Stay logically AwaitingIo.
            drop(scheduler_guard);
            crate::task::executor::halt();

            // Woke up from halt (likely timer or IO interrupt).
            // Check if we were woken properly.
            let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
            #[allow(static_mut_refs)]
            let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
            if let Some(process) = table.get_process(cur_pid) {
                if process.pending_io {
                    process.pending_io = false;
                    if matches!(process.state, ProcessState::Runnable) {
                        process.state = ProcessState::Running;
                    }
                    return;
                }
                if matches!(process.state, ProcessState::Runnable) {
                    // No context switch occurred: this task is still the BSP
                    // current task and is resuming directly after HLT.
                    process.state = ProcessState::Running;
                    return;
                }
            }
        }
    }
}

pub fn wake_process(pid: u16) {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };

    let Some(process) = table.get_process(pid) else {
        return;
    };

    match process.state {
        ProcessState::AwaitingIo => {
            process.state = ProcessState::Runnable;
            // Mark the wake so the just-unblocked task, once rescheduled and
            // re-entering await_io()'s loop top, can distinguish a genuine wake
            // from a first-entry block. Without this, the loop would clobber the
            // Runnable state back to AwaitingIo and lose the wakeup.
            process.pending_io = true;
        }
        ProcessState::Dead | ProcessState::Stopped => {}
        _ => {
            process.pending_io = true;
        }
    }
}

/// Wake a deadline-blocked process from the timer expiry path (#66).
///
/// The `token` must match the process's currently published `wait_token`; a
/// mismatch (stale/cancelled entry, or PID reuse) is a no-op. This is the
/// stale-entry guard that prevents a cancelled wait from waking a later one.
/// Called from hard-IRQ context by `sys::timer::expire_due` after the queue
/// guard has been released.
pub fn wake_from_timer(pid: u16, token: crate::sys::timer::WaitToken) {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { PROCESS_TABLE.get_mut() }) else {
        return;
    };
    let Some(process) = table.get_process(pid) else {
        return;
    };
    // Only wake a Sleeping process whose published token matches. A stale or
    // already-woken entry cannot wake a later wait.
    if process.wait_token != Some(token) || !matches!(process.state, ProcessState::Sleeping) {
        return;
    }
    process.state = ProcessState::Runnable;
    process.wait_token = None;
    process.wait_deadline_ns = None;
    process.wake_reason = Some(crate::sys::timer::WakeReason::Deadline);
}

/// Clear any outstanding wait token on a process about to exit or be killed.
/// The queue entry is reclaimed lazily when it reaches the heap head; clearing
/// the published token here makes it stale so it cannot wake a later wait.
fn invalidate_wait_token(process: &mut Process) {
    process.wait_token = None;
    process.wait_deadline_ns = None;
    process.wake_reason = None;
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

/// Adopt direct children of an exiting process into PID 1. Dead children stay
/// as zombies and are reported to init so they can be reaped there.
pub(crate) fn reparent_children(processes: &mut [Process], exiting_pid: u16) {
    if exiting_pid == 1 || !processes.iter().any(|process| process.pid == 1) {
        return;
    }

    let mut adopted_zombie = false;
    for process in processes.iter_mut() {
        if !process.is_thread && process.parent_pid == exiting_pid {
            process.parent_pid = 1;
            adopted_zombie |= matches!(process.state, ProcessState::Dead);
        }
    }

    if adopted_zombie
        && let Some(init) = processes.iter_mut().find(|process| process.pid == 1)
    {
        init.queue_signal(SIGCHLD);
    }
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
        if matches!(processes[idx].state, ProcessState::Runnable) {
            return Some(idx);
        }
    }

    None
}

/// Validate the BSP invariant at a stable scheduler boundary: exactly one
/// process-table entry is Running, and it is the process selected for the CPU.
fn validate_running_owner(processes: &[Process], expected_idx: usize) -> bool {
    let mut running = processes
        .iter()
        .enumerate()
        .filter(|(_, process)| matches!(process.state, ProcessState::Running));
    let first = running.next().map(|(idx, _)| idx);
    let extra = running.next().map(|(_, process)| process.pid);

    if first != Some(expected_idx) || extra.is_some() {
        crate::serial_println!(
            "[sched] Running invariant violated: expected_pid={} first_running={:?} extra_running={:?}",
            processes[expected_idx].pid,
            first.map(|idx| processes[idx].pid),
            extra,
        );
        return false;
    }
    true
}

/// Before selecting another task, no process other than the physical BSP
/// current process may be marked Running. The current process may already be
/// blocked or dead, in which case no Running entry is expected yet.
fn validate_pre_switch_owner(processes: &[Process], current_idx: usize) -> bool {
    if let Some((idx, process)) = processes
        .iter()
        .enumerate()
        .find(|(_, process)| matches!(process.state, ProcessState::Running))
        && idx != current_idx
    {
        crate::serial_println!(
            "[sched] non-current process marked Running: current_pid={} running_pid={}",
            processes[current_idx].pid,
            process.pid,
        );
        return false;
    }

    let count = processes
        .iter()
        .filter(|process| matches!(process.state, ProcessState::Running))
        .count();
    if count > 1 {
        crate::serial_println!("[sched] multiple Running tasks on BSP: count={}", count);
        return false;
    }
    true
}

fn switch_tasks_with_new_scheduler_guard(current: &mut Process, next: &mut Process) -> bool {
    let Some(scheduler_guard) = crate::sys::preempt::SchedulerGuard::try_enter() else {
        return false;
    };
    if !matches!(next.state, ProcessState::Runnable) {
        crate::serial_println!(
            "[sched] refused non-Runnable target pid={} state={:?}",
            next.pid,
            next.state,
        );
        return false;
    }

    if matches!(current.state, ProcessState::Running) {
        current.state = ProcessState::Runnable;
    }
    next.state = ProcessState::Running;
    crate::sys::preempt::clear_need_resched();
    scheduler_guard.release_before_switch();
    switch_tasks(current, next);
    true
}

fn switch_by_index_guarded(
    processes: &mut [Process],
    cur_idx: usize,
    next_idx: usize,
    scheduler_guard: crate::sys::preempt::SchedulerGuard,
) -> bool {
    if cur_idx == next_idx {
        return false;
    }
    if !validate_pre_switch_owner(processes, cur_idx) {
        return false;
    }
    if !matches!(processes[next_idx].state, ProcessState::Runnable) {
        crate::serial_println!(
            "[sched] refused non-Runnable target pid={} state={:?}",
            processes[next_idx].pid,
            processes[next_idx].state,
        );
        return false;
    }

    if matches!(processes[cur_idx].state, ProcessState::Running) {
        processes[cur_idx].state = ProcessState::Runnable;
    }
    processes[next_idx].state = ProcessState::Running;
    if !validate_running_owner(processes, next_idx) {
        return false;
    }

    let ptr = processes.as_mut_ptr();
    unsafe {
        let cur = &mut *ptr.add(cur_idx);
        let next = &mut *ptr.add(next_idx);

        PID.store(next.pid, Ordering::SeqCst);
        // switch_tasks() disables interrupts around the architecture switch.
        // Release CPU-local scheduler bookkeeping first because the next task
        // may enter userspace without returning through this Rust stack.
        scheduler_guard.release_before_switch();
        switch_tasks(cur, next);
    }
    true
}

/// Idle spin for a task that has no runnable alternative *yet*.
///
/// Used by the exit/kill paths when the current task is already Dead (or about
/// to be) but no other task is currently Runnable. This happens now that long
/// sleeps genuinely block (`Sleeping`) instead of busy-waiting as `Runnable`:
/// the only other task may be asleep and will be woken by a future timer IRQ.
///
/// A bare `loop { halt(); }` would deadlock here: the timer ISR sees
/// `from_user == 0` (this is kernel mode) and, with kernel preemption disabled,
/// cannot switch to the task it just woke. So after each `halt` we re-enter the
/// scheduler: once a timer expiry flips a sleeper to `Runnable`,
/// [`schedule_now`] switches to it and this task never returns.
///
/// The caller must have already dropped its `SchedulerGuard` so [`schedule_now`]
/// can re-enter the scheduler.
fn idle_until_runnable() -> ! {
    loop {
        crate::task::executor::halt();
        // Re-check for a runnable task. schedule_now() switches away if one
        // exists; if not, it returns false and we halt again.
        let _ = schedule_now();
    }
}

fn timer_preempt_common(frame: *mut PreemptFrame, from_user: u64) -> *mut PreemptFrame {
    // need_resched is already set by on_timer_tick() (called via
    // driver::time::handle_timer_event before this function). The logic below
    // decides whether to act on it now.

    // Drain deadline-queue overflow outside hard-IRQ context. expire_due()
    // (called from handle_timer_event under irq_enter) processes a bounded
    // batch and defers the remainder; finish that work now that irq_exit() has
    // restored task-context preemption rules. Woken sleepers are Runnable, so
    // the schedule_now() calls below will pick them up.
    crate::sys::timer::process_deferred_expiry();

    // Existing safe path: interrupted userspace → schedule immediately.
    if from_user != 0 {
        schedule_now();
        return frame;
    }

    // Experimental kernel-mode preemption. Disabled unless the compile-time
    // flag is set. When enabled, schedule only if every safety condition in
    // can_preempt_kernel() is satisfied.
    if crate::sys::preempt::ENABLE_KERNEL_PREEMPTION {
        if crate::sys::preempt::can_preempt_kernel() {
            let rip = unsafe { (*frame).rip };
            let rsp = unsafe { (*frame).rsp };
            // crate::serial_println!(
            //     "[kpreempt] allow: pid={} rip={:#x} rsp={:#x}",
            //     id(),
            //     rip,
            //     rsp,
            // );
            schedule_now();
        }
        // When can_preempt_kernel() returns false it has already logged the
        // skip reason (gated by KPREEMPT_DEBUG). need_resched remains set and
        // will be honored at the next cond_resched() safe point.
    }

    frame
}

pub extern "C" fn timer_preempt(frame: *mut PreemptFrame, from_user: u64) -> *mut PreemptFrame {
    crate::sys::preempt::irq_enter();
    crate::driver::time::handle_timer_event();

    // EOI for IRQ0 (PIC timer)
    unsafe {
        crate::arch::x86_64::idt::PICS
            .lock()
            .notify_end_of_interrupt(crate::arch::x86_64::idt::PIC_1_OFFSET);
    }

    crate::sys::preempt::irq_exit();
    timer_preempt_common(frame, from_user)
}

pub extern "C" fn apic_timer_preempt(
    frame: *mut PreemptFrame,
    from_user: u64,
) -> *mut PreemptFrame {
    crate::sys::preempt::irq_enter();
    crate::driver::time::handle_timer_event();

    // EOI for Local APIC
    crate::driver::apic::lapic::end_of_interrupt();

    crate::sys::preempt::irq_exit();
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
        // If the target frame returns to ring 3, make GS active for user mode.
        // Kernel mode keeps active GS pointing at KernelGsData.
        "push rax",
        "mov rax, [rsp + 16]",
        "and rax, 3",
        "cmp rax, 3",
        "jne 2f",
        "swapgs",
        "2:",
        "pop rax",
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
    USER_ENV.lock().push(String::from("TERM=xterm-256color"));
    let (page_table_frame, _) = Cr3::read();
    let page_table = crate::memory::create_page_table(page_table_frame);
    let mapper = unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

    // PID 0 belongs to the kernel bootstrap context. NEXT_PID remains at 1 so
    // the first userspace process is the Unix init process (PID 1).
    let pid = 0;
    PID.store(pid, Ordering::SeqCst);

    let proc_mm = Arc::new(Mutex::new(ProcMM::new(0)));

    let switch_stack = allocate_switch_stack().unwrap().as_mut_ptr::<u8>();

    let kernel_rsp = switch_stack as u64;
    let mut stack_ptr = initial_context_stack_top(kernel_rsp);

    let mut kgs = Box::new(KernelGsData {
        kernel_rsp: 0,
        user_rsp: 0,
    });

    kgs.kernel_rsp = kernel_rsp;

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
                tgid: pid,
                is_thread: false,
                addr_size_vec: Vec::new(),
                exe_path: "[kernel]".to_string(),
                comm: task_comm_from_path("[kernel]"),
                stack: 0,
                stack_size: 0,
                entry_point: 0,
                state: ProcessState::Runnable,
                page_table_frame,
                mapper,
                pwd: "/".to_string(),
                fd_table: Vec::new(),
                kernel_gs: kgs,
                gs_base: VirtAddr::zero(),
                fs_base: VirtAddr::zero(),
                proc_mm,
                parent_pid: 0,
                pgid: pid,
                sid: pid,
                umask: 0o022,
                exit_code: 0,
                wait_status: 0,
                wait_reported: false,
                preempt_frame: 0,
                pending_io: false,
                signal_actions: [SignalAction::default(); SIGNAL_COUNT],
                signal_mask: [0; 2],
                pending_signals: 0,
                sigsuspend_saved_mask: [0; 2],
                in_sigsuspend: false,
                signal_alt_stack: SignalAltStack::default(),
                wait_token: None,
                wait_deadline_ns: None,
                wake_reason: None,
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
        proc_mm: Arc::new(Mutex::new(ProcMM::new(0))),
        parent_pid: 0,
        pgid: 0,
        sid: 0,
        pid: 0,
        tgid: 0,
        is_thread: false,
        pwd: String::from("/"),
        context_switch_rsp: VirtAddr::zero(),
        mapper,
        fpu_storage: Some(FpuState::default()),
        entry_point: 0,
        addr_size_vec: Vec::new(),
        exe_path: "[idle]".to_string(),
        comm: task_comm_from_path("[idle]"),
        page_table_frame: f,
        fd_table: Vec::new(),
        state: ProcessState::Running,
        stack_size: 0,
        umask: 0o022,
        exit_code: 0,
        wait_status: 0,
        wait_reported: false,
        preempt_frame: 0,
        pending_io: false,
        signal_actions: [SignalAction::default(); SIGNAL_COUNT],
        signal_mask: [0; 2],
        pending_signals: 0,
        sigsuspend_saved_mask: [0; 2],
        in_sigsuspend: false,
        signal_alt_stack: SignalAltStack::default(),
        wait_token: None,
        wait_deadline_ns: None,
        wake_reason: None,
    };

    idle_task.gs_base = VirtAddr::new(&*idle_task.kernel_gs as *const _ as u64);

    #[allow(static_mut_refs)]
    let proc = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(0).unwrap() };

    if !switch_tasks_with_new_scheduler_guard(&mut idle_task, proc) {
        crate::serial_println!("[sched] initial process switch prevented");
        loop {
            crate::task::executor::halt();
        }
    }
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
    has_tls: bool,
    interp_path: Option<String>,
}

fn load_elf_image(
    executable: &mut VfsNode,
    elf_regions: &mut Vec<ElfRegion>,
    dyn_base_hint: Option<u64>,
    capture_interp: bool,
) -> Result<LoadedImage, ()> {
    let mut header_bytes = [0u8; size_of::<Elf64Ehdr>()];
    executable.read_exact(0, &mut header_bytes)?;
    // SAFETY: `header_bytes` contains exactly one ELF header and unaligned
    // reads are permitted by `read_unaligned`.
    let eh = unsafe { core::ptr::read_unaligned(header_bytes.as_ptr().cast::<Elf64Ehdr>()) };
    if eh.e_ident.get(0..4) != Some(&ELF_MAGIC) {
        return Err(());
    }

    let e_phoff = eh.e_phoff;
    let e_phentsize = eh.e_phentsize as u64;
    let e_phnum = eh.e_phnum as u64;
    if e_phentsize < core::mem::size_of::<Elf64Phdr>() as u64 {
        return Err(());
    }
    if e_phoff
        .checked_add(e_phentsize.saturating_mul(e_phnum))
        .map_or(true, |end| end as usize > executable.metadata.size)
    {
        return Err(());
    }

    let mut program_headers = Vec::with_capacity(e_phnum as usize);
    for i in 0..e_phnum {
        let offset = e_phoff
            .checked_add(i.checked_mul(e_phentsize).ok_or(())?)
            .ok_or(())?;
        let mut bytes = [0u8; size_of::<Elf64Phdr>()];
        executable.read_exact(offset as usize, &mut bytes)?;
        // SAFETY: `bytes` contains one complete program header and the ELF
        // format does not require the source buffer to be naturally aligned.
        let header = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<Elf64Phdr>()) };
        program_headers.push(header);
    }

    let mut load_bias = 0;
    if eh.e_type == 3 {
        load_bias = dyn_base_hint.unwrap_or(MAIN_DYN_LOAD_BASE);
    }

    let mut phdr_va = 0;
    let mut interp_path = None;
    let mut has_tls = false;

    for ph in &program_headers {
        if ph.p_type == PT_PHDR {
            phdr_va = load_bias + ph.p_vaddr;
        } else if capture_interp && ph.p_type == PT_INTERP {
            let start = ph.p_offset as usize;
            let len = ph.p_filesz as usize;
            if start
                .checked_add(len)
                .is_none_or(|end| end > executable.metadata.size)
            {
                return Err(());
            }
            let mut bytes = vec![0u8; len];
            executable.read_exact(start, &mut bytes)?;
            if let Ok(s) = core::str::from_utf8(&bytes) {
                interp_path = Some(s.trim_end_matches('\0').to_string());
            }
        } else if ph.p_type == PT_TLS && ph.p_memsz != 0 {
            has_tls = true;
        }
    }

    if phdr_va == 0 {
        let ph_tbl_start = e_phoff;
        let ph_tbl_end = e_phoff + e_phentsize * e_phnum;
        for ph in &program_headers {
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
    for ph in &program_headers {
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        if ph.p_filesz > ph.p_memsz {
            return Err(());
        }
        if ph
            .p_offset
            .checked_add(ph.p_filesz)
            .map_or(true, |end| end as usize > executable.metadata.size)
        {
            return Err(());
        }

        let seg_start = load_bias.checked_add(ph.p_vaddr).ok_or(())?;
        let seg_mem_end = seg_start.checked_add(ph.p_memsz).ok_or(())?;
        let map_start = align_down_u64(seg_start, 4096);
        let map_end = align_up_u64(seg_mem_end, 4096).ok_or(())?;
        let map_len = map_end.checked_sub(map_start).ok_or(())? as usize;
        let file_base = align_down_u64(ph.p_offset, 4096);

        if (ph.p_offset & (PAGE_SIZE_U64 - 1)) != (seg_start & (PAGE_SIZE_U64 - 1)) {
            return Err(());
        }

        if seg_mem_end > max_end {
            max_end = seg_mem_end;
        }

        elf_regions.push(ElfRegion {
            base: map_start as usize,
            len: map_len,
            file_base: map_start as usize,
            file_offset: file_base as usize,
            file_end: seg_start.checked_add(ph.p_filesz).ok_or(())? as usize,
            permissions: VmPermissions {
                read: ph.p_flags & PF_R != 0,
                write: ph.p_flags & PF_W != 0,
                execute: ph.p_flags & PF_X != 0,
            },
            file: executable.clone(),
        });
    }

    Ok(LoadedImage {
        entry_point: eh.e_entry + load_bias,
        phdr_va,
        phent: e_phentsize,
        phnum: e_phnum,
        max_end,
        load_base: load_bias,
        has_tls,
        interp_path,
    })
}

fn reserve_static_tls_spill(
    mapper: &mut OffsetPageTable<'_>,
    addr_size_vec: &mut Vec<(u64, usize)>,
    max_end: u64,
) -> Result<u64, ()> {
    let spill_start = align_up_u64(max_end, PAGE_SIZE_U64).ok_or(())?;
    let spill_len = PAGE_SIZE_U64
        .checked_mul(STATIC_TLS_SPILL_PAGES)
        .ok_or(())?;
    alloc_pages_unflushed(mapper, spill_start, spill_len as usize, true, false)?;
    addr_size_vec.push((spill_start, spill_len as usize));
    spill_start.checked_add(spill_len).ok_or(())
}

fn load_interpreter_image(path: &str, elf_regions: &mut Vec<ElfRegion>) -> Result<LoadedImage, ()> {
    #[allow(static_mut_refs)]
    let mut executable = {
        let vfs = unsafe { VFS.read() };
        vfs.open(path).map_err(|_| ())?
    };

    load_elf_image(
        &mut executable,
        elf_regions,
        Some(INTERP_DYN_LOAD_BASE),
        false,
    )
}

fn align_down_u64(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    Some((value.checked_add(align - 1)?) & !(align - 1))
}

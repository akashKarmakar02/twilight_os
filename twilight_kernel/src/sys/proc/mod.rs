pub mod mem;

use alloc::boxed::Box;
use crate::arch::x86_64::gdt::{USER_CS, USER_SS};
use crate::arch::x86_64::io::{IA32_FS_BASE, IA32_GS_BASE, set_fsbase, set_inactive_gsbase, wrmsr};
use crate::kernel_utils::exec::jump_to_user;
use crate::{println, serial_prtinln};
use crate::sys::console::init_console;
use crate::sys::fs::vfs::VfsNode;
use crate::sys::memory::{active_level_4_table, alloc_pages, dealloc_pages, frame_allocator, phys_mem_offset};
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use object::{Object, ObjectSegment, SegmentFlags};
use spin::Once;
use spin::mutex::Mutex;
use x86_64::VirtAddr;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{FrameAllocator, OffsetPageTable, PhysFrame};
use crate::sys::proc::mem::ProcMM;

pub static mut PROCESS_TABLE: Once<ProcessTable> = Once::new();

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
pub const USER_STACK_SIZE: usize = 0x1024000;
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

    pub fn run(&mut self, process: Process) {
        let pid = process.pid;

        PID.store(pid, Ordering::SeqCst);

        self.proc_list.push_back(process);
        self.proc_list.back_mut().unwrap().exec();
    }
}
pub struct Handler {
    pub handler: Arc<Mutex<VfsNode>>,
    pub seek: usize,
    pub path: String,
    pub flags: i32,
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
    pub proc_mm: Box<ProcMM>,
}

impl Process {
    pub fn new(content_buf: Vec<u8>, pwd: &str, args: &[&str]) -> Result<Self, ()> {
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

        let mut mapper =
            unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

        let user_stack_top = VirtAddr::new(USER_STACK_TOP);

        let mut entry_point_addr: u64 = 0;
        let mut phdr_va: u64 = 0;
        let mut phent: u64 = 0;
        let mut phnum: u64 = 0;
        let mut max_end: u64 = 0;

        if content_buf.get(0..4) == Some(&ELF_MAGIC) {
            if let Ok(obj) = object::File::parse(content_buf.as_slice()) {
                let eh = unsafe { &*(content_buf.as_ptr() as *const Elf64Ehdr) };
                let e_phoff = eh.e_phoff;
                let e_phentsize = eh.e_phentsize as u64; // 56
                let e_phnum = eh.e_phnum as u64;

                let load_bias: u64 = if eh.e_type == 3 /* ET_DYN */ { 0x400000 } else { 0 };
                phdr_va = 0;

                // Try PT_PHDR first
                for i in 0..e_phnum {
                    let ph = unsafe {
                        &*(content_buf
                            .as_ptr()
                            .add((e_phoff + i * e_phentsize) as usize)
                            as *const Elf64Phdr)
                    };
                    if ph.p_type == PT_PHDR {
                        phdr_va = load_bias + ph.p_vaddr;
                        break;
                    }
                }

                // Fallback: translate file offset via containing PT_LOAD
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

                entry_point_addr = eh.e_entry + load_bias;
                phent = e_phentsize;
                phnum = e_phnum;

                for segment in obj.segments() {
                    if let Ok(data) = segment.data() {
                        let addr = segment.address();
                        let size = segment.size() as usize;

                        let seg_end = addr + size as u64;
                        if seg_end > max_end {
                            max_end = seg_end;
                        }

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

                let user_stack_base = user_stack_top.as_u64() - USER_STACK_SIZE as u64;
                if let Ok(_) =
                    alloc_pages(&mut mapper, user_stack_base, USER_STACK_SIZE, true, false)
                {
                    addr_size_vec.push((user_stack_base, USER_STACK_SIZE));
                }
            } else {
                println!("ksh: invalid ELF file");
                return Err(());
            }
        }

        // Some(virt_to_phys(VirtAddr::new(0x400000)).unwrap().as_u64())
        let user_rsp = build_initial_stack(
            user_stack_top.as_u64(),
            entry_point_addr,
            Some(args),
            None,
            phdr_va,
            phent,
            phnum,
            None,
            None,
        );

        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let proc_mm = Box::new(ProcMM::new(max_end as usize));

        let p = Process {
            stack: user_rsp,
            stack_size: USER_STACK_SIZE,
            entry_point: entry_point_addr,
            pid,
            mapper,
            page_table_frame,
            state: ProcessState::Running,
            addr_size_vec,
            pwd: pwd.to_string(),
            fs_base: VirtAddr::zero(),
            gs_base: VirtAddr::zero(),
            handler: Vec::new(),
            proc_mm,
        };
        Ok(p)
    }

    pub fn exec(&self) {
        wrmsr(IA32_FS_BASE, VirtAddr::zero().as_u64());
        wrmsr(IA32_GS_BASE, VirtAddr::zero().as_u64());
        jump_to_user(
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

    #[allow(static_mut_refs)]
    unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .proc_list
            .push_back(Process {
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
                proc_mm,
            })
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
    entry_point: u64,
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
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()); }
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

    // ---- write auxv (topmost among these tables) ----
    let aux_vec: Vec<AuxvEntry> = vec![
        AuxvEntry { key: 3, value: phdr_addr }, // AT_PHDR
        AuxvEntry { key: 4, value: phent     }, // AT_PHENT
        AuxvEntry { key: 5, value: phnum     }, // AT_PHNUM
        AuxvEntry { key: 6, value: 4096      }, // AT_PAGESZ
        AuxvEntry { key: 9, value: entry_point }, // AT_ENTRY
        AuxvEntry { key: 25, value: rand_ptr }, // AT_RANDOM
        AuxvEntry { key: 31, value: execfn   }, // AT_EXECFN
        AuxvEntry { key: 17, value: 0 }, // AT_UID
        AuxvEntry { key: 18, value: 0 }, // AT_EUID
        AuxvEntry { key: 19, value: 0 }, // AT_GID
        AuxvEntry { key: 20, value: 0 }, // AT_EGID
        AuxvEntry { key: 23, value: 100 }, // AT_CLKTCK
        AuxvEntry { key: 0, value: 0 }, // AT_NULL
    ];

    rsp -= (core::mem::size_of::<AuxvEntry>() * aux_vec.len()) as u64;
    unsafe {
        core::ptr::copy_nonoverlapping(aux_vec.as_ptr(), rsp as *mut AuxvEntry, aux_vec.len());
    }

    rsp -= 8;
    unsafe {
        *(rsp as *mut u64) = 0;
    }

    // ---- envp pointers then NULL ----
    for &p in &envp_ptrs {
        rsp -= 8;
        unsafe {
            *(rsp as *mut u64) = p;
        }
    }

    // envp termintor
    rsp -= 8;
    unsafe {
        *(rsp as *mut u64) = 0;
    }


    // ---- argv pointers then NULL ----
    for &p in &argv_ptrs {
        serial_prtinln!("ptr: {:#X}", p);
        rsp -= 8;
        unsafe {
            *(rsp as *mut u64) = p;
        }
    }

    // ---- padding BEFORE argc to ensure final %rsp == 8 ----
    // We want (rsp_after_argc % 16 == 8). After we push argc (8 bytes),
    // rsp will be (current_rsp - 8). So we need (current_rsp % 16 == 0).
    // If it's 8, push a padding 0 to flip it to 0.

    // ---- argc ----
    rsp -= 8;
    unsafe { *(rsp as *mut u64) = argv_ptrs.len() as u64; }

    rsp
}

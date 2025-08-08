use crate::arch::x86_64::gdt::{USER_CS, USER_SS};
use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs;
use crate::sys::memory::{alloc_pages, phys_mem_offset};
use core::arch::asm;
use object::{Object, ObjectSegment, SegmentFlags};
use x86_64::VirtAddr;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{OffsetPageTable, Translate};

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

pub fn main(args: &[&str]) {
    if args.len() < 1 {
        println!("Usage: exec <file>");
        return;
    }

    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    let inode = if pwd == "/" {
        1
    } else {
        fs.resolve_path(pwd).unwrap()
    };

    if let Some(inode) = fs.find_dir_entry(inode, args[0]).unwrap() {
        let content_buf = fs.read_file(inode).unwrap();

        let (page_table_frame, _) = Cr3::read();

        let page_table = crate::sys::memory::create_page_table(page_table_frame);

        let mut entry_point_addr: u64 = 0;
        let mut mapper =
            unsafe { OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset())) };

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

                const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
                const STACK_SIZE: u64 = 0x4000;

                let user_stack_top = VirtAddr::new(USER_STACK_TOP);
                let user_stack_base = user_stack_top - STACK_SIZE;

                alloc_pages(
                    &mut mapper,
                    user_stack_base.as_u64(),
                    STACK_SIZE as usize,
                    true, // writable
                    false // executable
                ).unwrap();

                if let Some(phys) = mapper.translate_addr(VirtAddr::new(0x4000c9)) {
                    println!("Mapped to: {:#x}", phys);
                } else {
                    println!("Not mapped!");
                }

                jump_to_user(
                    code_addr,
                    entry_point_addr,
                    user_stack_top.as_u64(),
                    USER_CS.bits() as u64,
                    USER_SS.bits() as u64,
                );
            }
        } else {
            let code_addr = 0x400000;
            alloc_pages(&mut mapper, code_addr, content_buf.len(), false, true).unwrap();

            let src = content_buf.as_ptr();
            let dst = code_addr as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(src, dst, content_buf.len());
                core::ptr::write_bytes(dst.add(content_buf.len()), 0, content_buf.len());
            }
            let user_stack_top = 0x4444_4455_0000u64;
            let stack_size = 0x32000; // 128 KiB

            alloc_pages(
                &mut mapper,
                user_stack_top - stack_size,
                stack_size as usize,
                true,
                false,
            )
            .unwrap();

            jump_to_user(code_addr, entry_point_addr, user_stack_top, USER_CS.bits() as u64, USER_SS.bits() as u64);
        }
    }
}

pub fn jump_to_user(code_addr: u64, entry_point: u64, stack_top: u64, user_cs: u64, user_ss: u64) {
    use x86_64::registers::control::Cr3;

    // Load the new page table (must be done before entering user mode)
    let (_, flags) = Cr3::read();
    unsafe { Cr3::write(crate::sys::memory::get_page_table_frame(), flags) };

    let rip = code_addr + entry_point;
    unsafe {
        asm!(
        "cli",              // Disable interrupts
        "push {ss}",        // SS (user data segment)
        "push {stack}",     // RSP (stack pointer)
        "push 0x202",       // RFLAGS (IF = 1 | bit 1 always set)
        "push {cs}",        // CS (user code segment)
        "push {rip}",       // RIP (entry point)
        "iretq",            // Return to ring 3
        ss = in(reg) user_ss,
        stack = in(reg) stack_top,
        cs = in(reg) user_cs,
        rip = in(reg) rip,
        options(noreturn)
        );
    }
}

#[allow(dead_code)]
fn read_binary(vaddr: u64, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(vaddr as *const u8, len) }
}

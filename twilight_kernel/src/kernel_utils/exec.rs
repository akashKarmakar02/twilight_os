use core::arch::asm;
use crate::sys::console::DIR;
use crate::sys::fs;
use crate::sys::memory::{alloc_pages, mapper, phys_mem_offset};
use crate::println;
use object::{Object, ObjectSegment, ReadRef};
use x86_64::structures::paging::{FrameAllocator, OffsetPageTable};
use x86_64::VirtAddr;
use crate::arch::x86_64::gdt::GDT;

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

pub fn main(args: &[&str]) {
    if args.len() < 1 {
        println!("Usage: exec <file>");
        return;
    }

    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    let inode = if pwd == "/" { 1 } else { fs.resolve_path(pwd).unwrap() };

    if let Some(inode) = fs.find_dir_entry(inode, args[0]).unwrap() {
        let content_buf = fs.read_file(inode + 1).unwrap();

        let page_table_frame = crate::sys::memory::frame_allocator().allocate_frame().unwrap();

        let page_table = unsafe {
            crate::sys::memory::create_page_table(page_table_frame)
        };



        let mut entry_point_addr: u64 = 0;
        let mut mapper = unsafe {
            OffsetPageTable::new(page_table, VirtAddr::new(phys_mem_offset()))
        };

        let code_addr = 0x_4444_4444_0000;

        if content_buf[0..4] == ELF_MAGIC {
            if let Ok(obj) = object::File::parse(content_buf.as_slice()) {
                entry_point_addr = obj.entry();

                for segment in obj.segments() {
                    if let Ok(data) = segment.data() {
                        // NOTE: The size of the segment in memory can be
                        // larger than on the disk because the object can
                        // contain uninitialized sections like ".bss" that has
                        // a length but no data.
                        let addr = code_addr + segment.address();
                        let size = segment.size() as usize;

                        println!("Debug: data size: {}, size: {}", data.len(), size);
                        println!("Debug: segment: {:?}", segment);

                        load_binary(&mut mapper, addr, size, data).unwrap();
                    }
                }

                let user_stack_top = 0x4444_5555_0000u64;
                let stack_size = 0x4000; // 16 KiB

                alloc_pages(&mut mapper, user_stack_top - stack_size, stack_size as usize).unwrap();

                jump_to_user(code_addr, entry_point_addr, user_stack_top, GDT.1.user_code_selector.0 as u64, GDT.1.user_data_selector.0 as u64);
            }
        }
    } else {
        println!("exec: {}: No such file or directory", args[0]);
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


fn load_binary(
    mapper: &mut OffsetPageTable,
    vaddr: u64,
    mem_size: usize,
    data: &[u8],
) -> Result<(), ()> {
    if alloc_pages(mapper, vaddr, mem_size).err().is_some() {
        println!("alloc_pages failed");
        return Ok(());
    };
    let src = data.as_ptr();
    let dst = vaddr as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(src, dst, data.len());
        if mem_size > data.len() {
            core::ptr::write_bytes(dst.add(data.len()), 0, mem_size - data.len());
        }
    }
    Ok(())
}

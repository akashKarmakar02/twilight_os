#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(let_chains)]
#![feature(decl_macro)]
#![feature(step_trait)]
#![feature(allocator_api)]
#![feature(stmt_expr_attributes)]
#![feature(sync_unsafe_cell)]
extern crate alloc;

#[macro_use]
extern crate twilight_proc;

pub mod arch;
pub mod driver;
pub mod kernel_utils;
pub mod task;
pub mod sys;
pub mod utils;

use core::arch::asm;
use core::cell::SyncUnsafeCell;
use core::sync::atomic::Ordering::SeqCst;
use limine::BaseRevision;
use limine::framebuffer::Framebuffer;
use limine::request::{FramebufferRequest, HhdmRequest, MemoryMapRequest, MpRequest, StackSizeRequest};
use limine::response::{HhdmResponse, MemoryMapResponse, MpResponse};

#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static STACK: StackSizeRequest = StackSizeRequest::new().with_size(0x1000 * 32); // 16KiB of stack for both the BSP and the APs

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP: SyncUnsafeCell<MemoryMapRequest> = SyncUnsafeCell::new(MemoryMapRequest::new());

#[used]
#[unsafe(link_section = ".requests")]
static MP: MpRequest = MpRequest::new();

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    unsafe {
        core::ptr::read_volatile(STACK.get_response().unwrap());
    }

    driver::cpu::init_cpu_x86_64();

    let mut framebuffer: Option<Framebuffer> = None;
    let mut hhdm_response: Option<&HhdmResponse> = None;
    let mut mp_response: Option<&MpResponse> = None;

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(fb) = framebuffer_response.framebuffers().next() {
            framebuffer = Some(fb);
        }
    }

    if let Some(hr) = HHDM_REQUEST.get_response() {
        hhdm_response = Some(hr);
    }

    #[allow(static_mut_refs)]
    let memory_map_response: &mut MemoryMapResponse = unsafe { &mut *MEMMAP.get() }.get_response_mut().unwrap();

    if let Some(mpr) = MP.get_response() {
        mp_response = Some(mpr);
    }

    init(
        &framebuffer.unwrap(),
        hhdm_response.unwrap(),
        memory_map_response,
        mp_response.unwrap(),
    );

    // println!("\x1b[96m                                                     ,,    ,,    ,,            ,,                                        ");
    // println!("                           MMP\"\"MM\"\"YMM                db  `7MM    db          `7MM        mm         .g8\"\"8q.    .M\"\"\"bgd ");
    // println!("                           P'   MM   `7                      MM                  MM        MM       .dP'    `YM. ,MI    \"Y ");
    // println!("                                MM `7M'    ,A    `MF'`7MM    MM  `7MM  .P\"Ybmmm  MMpMMMb.mmMMmm     dM'      `MM `MMb.     ");
    // println!("                                MM   VA   ,VAA   ,V    MM    MM    MM :MI  I8    MM    MM  MM       MM        MM   `YMMNq. ");
    // println!("                                MM    VA ,V  VA ,V     MM    MM    MM  WmmmP\"    MM    MM  MM       MM.      ,MP .     `MM ");
    // println!("                                MM     VVV    VVV      MM    MM    MM  8M         MM    MM  MM       `Mb.    ,dP' Mb     dM ");
    // println!("                              .JMML.    W      W     .JMML..JMML..JMML.YMMMMMb .JMML  JMML.`Mbmo      `\"bmmd\"'   P\"Ybmmd\"  ");
    // println!("                                                                       6'     dP                                             ");
    // println!("                                                                       Ybmmmd'                                               \x1b[0m");

    println!(r"___________       .__.__  .__       .__     __    ________    _________ ");
    println!(r"\__    ___/_  _  _|__|  | |__| ____ |  |___/  |_  \_____  \  /   _____/ ");
    println!(r"  |    |  \ \/ \/ /  |  | |  |/ ___\|  |  \   __\  /   |   \ \_____  \  ");
    println!(r"  |    |   \     /|  |  |_|  / /_/  |   Y  \  |   /    |    \/        \ ");
    println!(r"  |____|    \/\_/ |__|____/__\___  /|___|  /__|   \_______  /_______  / ");
    println!(r"                            /_____/      \/               \/        \/  ");

    sys::console::init_console();

    hcf()
}

#[panic_handler]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    serial_prtinln!("[PANIC]: {}", info);
    hcf();
}

fn hcf() -> ! {
    loop {
        unsafe {
            #[cfg(target_arch = "x86_64")]
            asm!("hlt");
            #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
            asm!("wfi");
            #[cfg(target_arch = "loongarch64")]
            asm!("idle 0");
        }
    }
}
use x86_64::VirtAddr;
use sys::{fs, memory};
use crate::sys::console::writer::init_writer;
use sys::framebuffer::init_framebuffer;
use crate::task::executor;

pub fn init(fb: &Framebuffer, hhdm_response: &HhdmResponse, memory_map_response: &'static mut MemoryMapResponse, mp_response: &'static MpResponse) {
    driver::uart::init();

    let phys_mem_offset = VirtAddr::new(hhdm_response.offset());

    #[allow(static_mut_refs)]
    unsafe {
        memory::PHYSICAL_MEMORY_OFFSET.store(phys_mem_offset.as_u64(), SeqCst);
    }

    memory::paging::init(memory_map_response).expect("paging init failed");

    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::idt::init_pics();

    memory::init(phys_mem_offset, memory_map_response.entries());


    init_framebuffer(fb);

    executor::init_executor();
    fs::init_fs();

    init_writer();
    sys::pci::init();

    // depends on pci initialization
    driver::nic::init();
    driver::usb::init();
    driver::cpu::init(mp_response);
    driver::disk::ata::init();
    fs::init(true);


    arch::x86_64::gdt::init_after_boot();

    arch::x86_64::syscall::init();

    sys::proc::init();

    x86_64::instructions::interrupts::enable();


    // #[allow(static_mut_refs)]
    // if let Some(fb) = unsafe { FRAMEBUFFER.get_mut() } {
    //     fb.animate_bouncing_rect(3000);
    //     fb.clear_buf(0x101010);
    //     fb.sync_full();
    // }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => ({
        let time = $crate::driver::timer::pit::uptime();
        $crate::println!("\x1b[93m[{:.6}]\x1b[0m {}", time, format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! logger {
    ($($arg:tt)*) => ({
        let time = $crate::driver::timer::pit::uptime();
        $crate::serial_prtinln!("\x1b[93m[{:.6}]\x1b[0m {}", time, format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! extern_sym {
    ($sym:ident) => {{
        unsafe extern "C" {
            static $sym: ::core::ffi::c_void;
        }

        // The value is not accessed, we only take its address. The `addr_of!()` ensures
        // that no intermediate references is created.
        ::core::ptr::addr_of!($sym)
    }};
}

#[alloc_error_handler]
fn alloc_error(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}

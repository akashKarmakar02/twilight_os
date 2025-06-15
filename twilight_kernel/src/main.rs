#![no_std]
#![no_main]
extern crate alloc;

use core::arch::asm;
use limine::BaseRevision;
use limine::framebuffer::Framebuffer;
use limine::request::{FramebufferRequest, HhdmRequest, MemoryMapRequest, MpRequest};
use limine::response::{HhdmResponse, MemoryMapResponse, MpResponse};
use twilight_kernel::driver::keyboard::keyboard_interrupt;
use twilight_kernel::task::Task;
use twilight_kernel::task::executor::EXECUTOR;
use twilight_kernel::{serial_prtinln, serial_print, println};

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
static MEMMAP: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MP: MpRequest = MpRequest::new();

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    let mut framebuffer: Option<Framebuffer> = None;
    let mut hhdm_response: Option<&HhdmResponse> = None;
    let mut memory_map_response: Option<&MemoryMapResponse> = None;
    let mut mp_response: Option<&MpResponse> = None;

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(fb) = framebuffer_response.framebuffers().next() {
            framebuffer = Some(fb);
        }
    }

    if let Some(hr) = HHDM_REQUEST.get_response() {
        hhdm_response = Some(hr);
    }

    if let Some(mmr) = MEMMAP.get_response() {
        memory_map_response = Some(mmr);
    }

    if let Some(mpr) = MP.get_response() {
        mp_response = Some(mpr);
    }

    twilight_kernel::init(
        &framebuffer.unwrap(),
        hhdm_response.unwrap(),
        memory_map_response.unwrap(),
        mp_response.unwrap(),
    );
    
    println!("\x1b[96m                                                     ,,    ,,    ,,            ,,                                        ");
    println!("                           MMP\"\"MM\"\"YMM                db  `7MM    db          `7MM        mm         .g8\"\"8q.    .M\"\"\"bgd ");
    println!("                           P'   MM   `7                      MM                  MM        MM       .dP'    `YM. ,MI    \"Y ");
    println!("                                MM `7M'    ,A    `MF'`7MM    MM  `7MM  .P\"Ybmmm  MMpMMMb.mmMMmm     dM'      `MM `MMb.     ");
    println!("                                MM   VA   ,VAA   ,V    MM    MM    MM :MI  I8    MM    MM  MM       MM        MM   `YMMNq. ");;
    println!("                                MM    VA ,V  VA ,V     MM    MM    MM  WmmmP\"    MM    MM  MM       MM.      ,MP .     `MM ");
    println!("                                MM     VVV    VVV      MM    MM    MM  8M         MM    MM  MM       `Mb.    ,dP' Mb     dM ");
    println!("                              .JMML.    W      W     .JMML..JMML..JMML.YMMMMMb .JMML  JMML.`Mbmo      `\"bmmd\"'   P\"Ybmmd\"  ");
    println!("                                                                       6'     dP                                             ");
    println!("                                                                       Ybmmmd'                                               \x1b[0m");

    twilight_kernel::console::init_console();
    
    twilight_kernel::console::start_kernel_console();

    let mut executor = EXECUTOR.get().unwrap().lock();
    executor.spawn(Task::new(keyboard_interrupt()));
    executor.run();
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

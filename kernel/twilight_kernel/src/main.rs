#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
#![feature(let_chains)]
#![feature(decl_macro)]
#![feature(slice_pattern)]
#![feature(step_trait)]
#![feature(allocator_api)]
#![feature(stmt_expr_attributes)]
#![feature(sync_unsafe_cell)]
#![feature(ip_from)]
extern crate alloc;
#[macro_use]
extern crate twilight_proc;

pub mod arch;
pub mod compat;
pub mod driver;
pub mod kernel_utils;
pub mod sys;
pub mod task;
pub mod utils;

use core::arch::asm;
use core::cell::SyncUnsafeCell;
use core::sync::atomic::Ordering::SeqCst;
use limine::BaseRevision;
use limine::framebuffer::Framebuffer;
use limine::request::{
    FramebufferRequest, HhdmRequest, MemoryMapRequest, ModuleRequest, MpRequest, StackSizeRequest,
};
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

#[used]
#[unsafe(link_section = ".requests")]
static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    unsafe {
        core::ptr::read_volatile(STACK.get_response().unwrap());
    }

    driver::cpu::init_cpu_x86_64();

    let mut framebuffer: Option<Framebuffer> = None;
    let mut hhdm_response: Option<&HhdmResponse> = None;
    let mut cpio_response: Option<&&limine::file::File> = None;
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
    let memory_map_response: &mut MemoryMapResponse =
        unsafe { &mut *MEMMAP.get() }.get_response_mut().unwrap();

    if let Some(module_response) = MODULE_REQUEST.get_response() {
        for module in module_response.modules() {
            if module.path().to_str().unwrap() == "/boot/initramfs.cpio" {
                // let cpio_buf = unsafe { core::slice::from_raw_parts(module.addr() as *const u8, module.size() as usize) };
                // let cpio = CpioIterator::new(cpio_buf);
                cpio_response = Some(module);
                break;
            }
        }
    }

    if let Some(mpr) = MP.get_response() {
        mp_response = Some(mpr);
    }

    init(
        &framebuffer.unwrap(),
        hhdm_response.unwrap(),
        memory_map_response,
        mp_response.unwrap(),
        cpio_response.unwrap(),
    );

    println!(
        "\x1b[96m                                                     ,,    ,,    ,,            ,,                                        "
    );
    println!(
        "                           MMP\"\"MM\"\"YMM                db  `7MM    db          `7MM        mm         .g8\"\"8q.    .M\"\"\"bgd "
    );
    println!(
        "                           P'   MM   `7                      MM                  MM        MM       .dP'    `YM. ,MI    \"Y "
    );
    println!(
        "                                MM `7M'    ,A    `MF'`7MM    MM  `7MM  .P\"Ybmmm  MMpMMMb.mmMMmm     dM'      `MM `MMb.     "
    );
    println!(
        "                                MM   VA   ,VAA   ,V    MM    MM    MM :MI  I8    MM    MM  MM       MM        MM   `YMMNq. "
    );
    println!(
        "                                MM    VA ,V  VA ,V     MM    MM    MM  WmmmP\"    MM    MM  MM       MM.      ,MP .     `MM "
    );
    println!(
        "                                MM     VVV    VVV      MM    MM    MM  8M         MM    MM  MM       `Mb.    ,dP' Mb     dM "
    );
    println!(
        "                              .JMML.    W      W     .JMML..JMML..JMML.YMMMMMb .JMML  JMML.`Mbmo      `\"bmmd\"'   P\"Ybmmd\"  "
    );
    println!(
        "                                                                       6'     dP                                             "
    );
    println!(
        "                                                                       Ybmmmd'                                               \x1b[0m"
    );

    // sys::proc::switch::switch_demo();
    // sys::console::framebuffer::init();
    // sys::console::init_console();

    sys::proc::init();

    hcf()
}

#[cfg(not(test))]
#[panic_handler]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
    crate::sys::kmsg::push_log(format_args!("[PANIC]: {}", info));
    serial_println!("[PANIC]: {}", info);
    hcf();
}

fn hcf() -> ! {
    loop {
        crate::driver::usb::poll_all_drivers();
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
use crate::kernel_utils::install::INITRAMFS;
use crate::sys::console::init_tty;
use crate::sys::fs::ram_fs::initramfs::CpioIterator;
use crate::sys::rng;
use crate::task::executor;
use sys::framebuffer::init_framebuffer;
use sys::{fs, memory};
use x86_64::VirtAddr;

pub fn init(
    fb: &Framebuffer,
    hhdm_response: &HhdmResponse,
    memory_map_response: &'static mut MemoryMapResponse,
    mp_response: &'static MpResponse,
    cpio_file: &&limine::file::File,
) {
    driver::uart::init();

    let phys_mem_offset = VirtAddr::new(hhdm_response.offset());

    #[allow(static_mut_refs)]
    unsafe {
        memory::PHYSICAL_MEMORY_OFFSET.store(phys_mem_offset.as_u64(), SeqCst);
    }

    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::idt::init_pics();

    // Now that the IDT and PICs are ready, wire COM1 receive interrupts into
    // the TTY so the host can drive the guest over serial.
    driver::uart::init_input_irq();

    memory::init(phys_mem_offset, memory_map_response.entries());

    init_framebuffer(fb);

    executor::init_executor();

    rng::init();

    // init_writer();
    init_tty();
    driver::mouse::ps2::init();
    sys::pci::init();

    // depends on pci initialization
    driver::freebsd_demo::init();
    driver::nic::init();
    driver::usb::init();
    driver::cpu::init(mp_response);
    driver::disk::init();

    let cpio_buf = unsafe {
        core::slice::from_raw_parts(cpio_file.addr() as *const u8, cpio_file.size() as usize)
    };
    let cpio = CpioIterator::new(cpio_buf);

    {
        *INITRAMFS.lock() = cpio;
    }

    fs::init(true);

    arch::x86_64::gdt::init_after_boot();

    arch::x86_64::syscall::init();

    // Discover and cache ACPI tables. The HPET clocksource (selected below when
    // the TSC is not validated) is discovered through these tables, so ACPI must
    // be initialized before the time subsystem. This also wires power control.
    sys::acpi::init();

    // Initialize the clocksource before anything that reads time or calibrates
    // against it, and before interrupts are enabled. Selection policy (#65):
    // validated invariant/paravirtual TSC, else HPET, else explicit failure.
    // The TSC frequency is published here for `timer::wait` and LAPIC calibration
    // below. This must precede `timer::init` and `lapic::init`.
    driver::time::init();

    driver::timer::init();

    // Switch to APIC timer. Calibration busy-waits on the selected clocksource
    // (TSC or HPET), not the interrupt-count clock, so it is correct under KVM
    // and TCG (#62/#68). The LAPIC is programmed in one-shot mode as a
    // clockevent; clockevent::init arms the first event.
    driver::apic::lapic::init();
    driver::time::clockevent::init();

    // Establish the realtime epoch from the CMOS RTC. The RTC defines the
    // wall-clock origin; elapsed time thereafter comes from the selected
    // clocksource (TSC or HPET).
    driver::time::init_realtime_offset_from_cmos();

    // Mask PIT (IRQ0) to prevent double ticks
    unsafe {
        let mut pics = arch::x86_64::idt::PICS.lock_irq();
        let masks = pics.read_masks();
        // Preserve all dynamically unmasked IRQs (e.g. UHCI IRQ10) and only mask IRQ0.
        pics.write_masks(masks[0] | 0x01, masks[1]);
    }

    // Complete PIT/LAPIC setup before BSP interrupts are globally enabled (#65).
    x86_64::instructions::interrupts::enable();

    kernel_utils::dhcp::main();
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => ({
        let time = $crate::driver::time::uptime_secs_f64();
        $crate::sys::kmsg::push_log(format_args!("[{:.6}] {}", time, format_args!($($arg)*)));
        $crate::serial_println!("\x1b[93m[{:.6}]\x1b[0m {}", time, format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! logger {
    ($($arg:tt)*) => ({
        let time = $crate::driver::time::uptime_secs_f64();
        $crate::sys::kmsg::push_log(format_args!("[{:.6}] {}", time, format_args!($($arg)*)));
        $crate::serial_println!("\x1b[93m[{:.6}]\x1b[0m {}", time, format_args!($($arg)*));
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

// #[alloc_error_handler]
// fn alloc_error(layout: alloc::alloc::Layout) -> ! {
//     panic!("allocation error: {:?}", layout);
// }

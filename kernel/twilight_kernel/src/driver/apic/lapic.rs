use crate::sys::memory::phys_mem_offset;
use core::ptr::{read_volatile, write_volatile};

// Standard Local APIC base address
const LAPIC_BASE: u64 = 0xFEE00_000;

// Register Offsets
const ID_REG: u64 = 0x020;
#[allow(dead_code)]
const VERSION_REG: u64 = 0x030;
#[allow(dead_code)]
const TPR_REG: u64 = 0x080; // Task Priority
const EOI_REG: u64 = 0x0B0; // End of Interrupt
const SVR_REG: u64 = 0x0F0; // Spurious Interrupt Vector
const TICR_REG: u64 = 0x380; // Timer Initial Count
const TCCR_REG: u64 = 0x390; // Timer Current Count
const TDCR_REG: u64 = 0x3E0; // Timer Divide Config
const LVT_TIMER_REG: u64 = 0x320; // LVT Timer

// Timer Modes
#[allow(dead_code)]
const TIMER_ONE_SHOT: u32 = 0x00;
const TIMER_PERIODIC: u32 = 0x20000;

pub unsafe fn write_reg(offset: u64, value: u32) {
    let base = phys_mem_offset() + LAPIC_BASE;
    let ptr = (base + offset) as *mut u32;
    unsafe {
        write_volatile(ptr, value);
    }
}

pub unsafe fn read_reg(offset: u64) -> u32 {
    let base = phys_mem_offset() + LAPIC_BASE;
    let ptr = (base + offset) as *const u32;
    unsafe { read_volatile(ptr) }
}

pub fn end_of_interrupt() {
    unsafe {
        write_reg(EOI_REG, 0);
    }
}

pub fn id() -> u32 {
    unsafe { read_reg(ID_REG) >> 24 }
}

pub fn init() {
    unsafe {
        // Map APIC MMIO page to prevent page fault
        crate::sys::memory::map_mmio(LAPIC_BASE, 4096).expect("Failed to map LAPIC");

        // Enable LAPIC (set bit 8 of SVR, and map vector 0xFF to spurious interrupts)
        write_reg(SVR_REG, 0x100 | 0xFF);

        // Mask timer interrupt for now
        write_reg(LVT_TIMER_REG, 0x10000);

        // Set Timer Divide Configuration to Divide by 16 (value: 0x3)
        // 0x3 means divide by 16 for standard APIC
        write_reg(TDCR_REG, 0x3);

        // Init timer calibration
        calibrate_timer();
    }
}

fn calibrate_timer() {
    // Calibrate the LAPIC timer interval against the TSC clocksource, not the
    // interrupt-count clock. The old code called `pit::sleep_ns(10ms)`, which
    // counted delivered timer interrupts; under KVM those can be coalesced,
    // producing an interval that is too long and a clock that runs slow for the
    // whole boot (#62). Busy-waiting on the TSC measures real elapsed time.

    unsafe {
        // One shot mode, large value
        write_reg(TICR_REG, 0xFFFFFFFF);
    }

    // Wait 10 ms of real time using the TSC.
    crate::driver::timer::wait(10_000_000);

    let ticks_in_10ms = unsafe {
        let current = read_reg(TCCR_REG);
        0xFFFFFFFF - current
    };

    let ticks_per_ms = ticks_in_10ms / 10;

    // Target: 1000Hz -> 1ms per tick
    let interval = ticks_per_ms; // 1 ms

    unsafe {
        // Periodic mode, vector 0xFD (as planned), unmasked
        write_reg(LVT_TIMER_REG, TIMER_PERIODIC | 0xFD);

        // Set divider again to be sure
        write_reg(TDCR_REG, 0x3);

        // Start timer
        write_reg(TICR_REG, interval);
    }
}

//! PIT (8253/8254) hardware: clock-event and calibration reference only.
//!
//! The PIT no longer owns the system timeline. `CLOCK_MONOTONIC` is read from
//! the invariant-TSC clocksource in [`crate::driver::time`]. The PIT is used
//! here only to program channel 0 (the legacy timer interrupt source used during
//! early boot and as a calibration reference) and as a wall-clock reference for
//! TSC frequency calibration.
//!
//! See issue #62 for why interrupt-count timekeeping was removed.

/// Program PIT channel 0 to a 1 kHz square wave (1 ms period).
///
/// Channel 0 is the legacy IRQ0 source. During boot it drives timer events
/// until the LAPIC timer takes over; afterwards IRQ0 is masked. The divisor
/// `1193182 / 1000 = 1193` yields ~1 kHz with a ~0.015% rate error, which is
/// negligible for a calibration reference and irrelevant to the TSC clocksource.
pub fn init() {
    let divisor: u16 = 1193;
    unsafe {
        use x86_64::instructions::port::Port;
        let mut command: Port<u8> = Port::new(0x43);
        let mut data: Port<u8> = Port::new(0x40);

        // 0x36: Channel 0, Access lo/hi, Mode 3 (Square wave), Binary
        command.write(0x36);
        data.write((divisor & 0xFF) as u8);
        data.write((divisor >> 8) as u8);
    }
}

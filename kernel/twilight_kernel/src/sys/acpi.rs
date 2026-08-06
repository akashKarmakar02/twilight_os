//! ACPI table discovery and power control.
//!
//! [`init`] performs ACPI discovery once at boot and caches the resulting
//! [`AcpiTables`] in [`ACPI_TABLES`] so that drivers (e.g. the HPET clocksource)
//! can derive information from the tables without rescanning. The cached tables
//! live for the lifetime of the kernel: `AcpiTables` borrows the RSDP/XSDT
//! memory, which is permanent firmware-owned RAM and is never reclaimed.

use crate::{println, sys};
use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};
use alloc::boxed::Box;
use aml::{AmlContext, AmlName, AmlValue, DebugVerbosity, Handler};
use core::ptr::NonNull;
use spin::Mutex;
use x86_64::PhysAddr;
use x86_64::instructions::port::Port;

static mut PM1A_CNT_BLK: u32 = 0;
static mut SLP_TYPA: u16 = 0;
static SLP_LEN: u16 = 0;

/// The system's ACPI tables, discovered once during [`init`] and cached for the
/// lifetime of the kernel.
///
/// `AcpiTables` is `Send` (its `PhysicalMapping` is `Send` when the handler is)
/// but not `Sync` (the mapping holds a `NonNull`), so it is stored behind a
/// [`Mutex`], which is `Sync` whenever `T: Send`. Drivers borrow it through
/// [`with_tables`] rather than rescanning for the RSDP themselves. The borrowed
/// firmware memory is permanent, so the `'static` lifetime is sound.
static ACPI_TABLES: Mutex<Option<AcpiTables<KernelAcpiHandler>>> = Mutex::new(None);

#[derive(Clone)]
pub struct KernelAcpiHandler;

unsafe impl Send for KernelAcpiHandler {}
unsafe impl Sync for KernelAcpiHandler {}

impl AcpiHandler for KernelAcpiHandler {
    #[allow(unsafe_code)]
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        // Firmware regions (BIOS ROM, EBDA, ACPI tables in reserved memory) are
        // not part of the usable-RAM HHDM map, so ensure they are mapped before
        // dereferencing. `ensure_physical_mapped` is a no-op for already-mapped
        // ranges.
        let virt_addr = sys::memory::ensure_physical_mapped(physical_address as u64, size)
            .expect("ACPI: failed to map physical region");
        let ptr = NonNull::new(virt_addr.as_mut_ptr()).unwrap();
        unsafe { PhysicalMapping::new(physical_address, ptr, size, size, Self) }
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}
}

/// Run a closure with the cached ACPI tables, if [`init`] has completed
/// successfully.
///
/// Drivers call this (e.g. to build `HpetInfo`) instead of rescanning for the
/// RSDP themselves. Returns `None` if ACPI discovery failed or has not run.
pub fn with_tables<R>(f: impl FnOnce(&AcpiTables<KernelAcpiHandler>) -> R) -> Option<R> {
    let guard = ACPI_TABLES.lock();
    guard.as_ref().map(f)
}

/// Discover and cache the ACPI tables, then set up power control (FADT/DSDT/S5).
///
/// Called once during boot, before any driver that consumes ACPI-derived
/// information (notably the HPET clocksource). After this returns, [`with_tables`]
/// is available.
pub fn init() {
    let res = unsafe { AcpiTables::search_for_rsdp_bios(KernelAcpiHandler) };
    match res {
        Ok(acpi) => {
            setup_power_control(&acpi);
            *ACPI_TABLES.lock() = Some(acpi);
        }
        Err(_e) => {
            println!("ACPI: Could not find RSDP in BIOS");
        }
    }
}

/// Extract FADT/DSDT/S5 sleep-state information for [`shutdown`].
fn setup_power_control(acpi: &AcpiTables<KernelAcpiHandler>) {
    if let Ok(fadt) = acpi.find_table::<acpi::fadt::Fadt>() {
        if let Ok(block) = fadt.pm1a_control_block() {
            unsafe {
                PM1A_CNT_BLK = block.address as u32;
            }
        }
    }
    if let Ok(dsdt) = acpi.dsdt() {
        // The DSDT lives in firmware/reserved memory that may not be part of the
        // usable-RAM HHDM map, so ensure it is mapped before reading. If mapping
        // fails, skip parsing rather than dereferencing an unmapped address — keep
        // the hardcoded `_S5` fallback so shutdown still works.
        let virt_addr =
            match sys::memory::ensure_physical_mapped(dsdt.address as u64, dsdt.length as usize)
            {
                Ok(v) => v,
                Err(()) => {
                    println!("ACPI: could not map DSDT; using hardcoded S5 fallback");
                    unsafe {
                        SLP_TYPA = (5 & 7) << 10;
                    }
                    return;
                }
            };
        let ptr = virt_addr.as_ptr();
        let table = unsafe { core::slice::from_raw_parts(ptr, dsdt.length as usize) };
        let handler = Box::new(KernelAmlHandler);
        let mut aml = AmlContext::new(handler, DebugVerbosity::None);
        if aml.parse_table(table).is_ok() {
            let name = AmlName::from_str("\\_S5").unwrap();
            let res = aml.namespace.get_by_path(&name);
            if let Ok(AmlValue::Package(s5)) = res {
                if let AmlValue::Integer(value) = s5[0] {
                    unsafe {
                        SLP_TYPA = value as u16;
                    }
                }
            }
        } else {
            println!("ACPI: Could not parse AML in DSDT");
            // FIXME: AML parsing works on QEMU and Bochs but not
            // on VirtualBox at the moment, so we use the following
            // hardcoded value:
            unsafe {
                SLP_TYPA = (5 & 7) << 10;
            }
        }
    } else {
        println!("ACPI: Could not find DSDT in BIOS");
    }
}

pub fn shutdown() {
    unsafe {
        let mut port: Port<u16> = Port::new(PM1A_CNT_BLK as u16);
        port.write(SLP_TYPA | SLP_LEN);
    }
}

struct KernelAmlHandler;

impl Handler for KernelAmlHandler {
    fn read_u8(&self, address: usize) -> u8 {
        read_addr::<u8>(address)
    }
    fn read_u16(&self, address: usize) -> u16 {
        read_addr::<u16>(address)
    }
    fn read_u32(&self, address: usize) -> u32 {
        read_addr::<u32>(address)
    }
    fn read_u64(&self, address: usize) -> u64 {
        read_addr::<u64>(address)
    }

    fn write_u8(&mut self, _: usize, _: u8) {
        unimplemented!()
    }
    fn write_u16(&mut self, _: usize, _: u16) {
        unimplemented!()
    }
    fn write_u32(&mut self, _: usize, _: u32) {
        unimplemented!()
    }
    fn write_u64(&mut self, _: usize, _: u64) {
        unimplemented!()
    }
    fn read_io_u8(&self, _: u16) -> u8 {
        unimplemented!()
    }
    fn read_io_u16(&self, _: u16) -> u16 {
        unimplemented!()
    }
    fn read_io_u32(&self, _: u16) -> u32 {
        unimplemented!()
    }
    fn write_io_u8(&self, _: u16, _: u8) {
        unimplemented!()
    }
    fn write_io_u16(&self, _: u16, _: u16) {
        unimplemented!()
    }
    fn write_io_u32(&self, _: u16, _: u32) {
        unimplemented!()
    }
    fn read_pci_u8(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u8 {
        unimplemented!()
    }
    fn read_pci_u16(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u16 {
        unimplemented!()
    }
    fn read_pci_u32(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u32 {
        unimplemented!()
    }
    fn write_pci_u8(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u8) {
        unimplemented!()
    }
    fn write_pci_u16(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u16) {
        unimplemented!()
    }
    fn write_pci_u32(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u32) {
        unimplemented!()
    }
}

fn read_addr<T>(addr: usize) -> T
where
    T: Copy,
{
    let virtual_address = sys::memory::phys_to_virt(PhysAddr::new(addr as u64));
    unsafe { *virtual_address.as_ptr::<T>() }
}

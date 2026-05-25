use core::mem::size_of;

/* =========================================================
   xHCI Capability Registers
   (Read-only, MMIO mapped)
========================================================= */

#[repr(C)]
pub struct XhciCapabilityRegisters {
    pub caplength: u8, // Capability Register Length
    pub reserved0: u8,
    pub hciversion: u16, // Interface Version Number
    pub hcsparams1: u32, // Structural Parameters 1
    pub hcsparams2: u32, // Structural Parameters 2
    pub hcsparams3: u32, // Structural Parameters 3
    pub hccparams1: u32, // Capability Parameters 1
    pub dboff: u32,      // Doorbell Offset
    pub rtsoff: u32,     // Runtime Register Space Offset
    pub hccparams2: u32, // Capability Parameters 2
}

/* Compile-time size check (equivalent to static_assert) */
const _: () = assert!(size_of::<XhciCapabilityRegisters>() == 32);

/* =========================================================
   xHCI Runtime Registers (subset)
========================================================= */

#[repr(C)]
pub struct XhciInterrupterRegisters {
    pub iman: u32,   // Interrupter Management
    pub imod: u32,   // Interrupter Moderation
    pub erstsz: u32, // Event Ring Segment Table Size
    pub reserved: u32,
    pub erstba: u64, // Event Ring Segment Table Base Address
    pub erdp: u64,   // Event Ring Dequeue Pointer
}

const _: () = assert!(size_of::<XhciInterrupterRegisters>() == 32);

#[repr(C)]
pub struct XhciRuntimeRegisters {
    pub mfindex: u32,
    pub reserved: [u32; 7],
    pub ir: [XhciInterrupterRegisters; 1], // we only use interrupter 0 for now
}

/* =========================================================
   Doorbells
========================================================= */

#[repr(C)]
pub struct XhciDoorbellRegisters {
    pub db: [u32; 256], // enough for xHCI doorbells
}

/* =========================================================
   xHCI Operational Registers
   (Read/write, MMIO mapped)
========================================================= */

#[repr(C)]
pub struct XhciOperationalRegisters {
    pub usbcmd: u32,   // USB Command
    pub usbsts: u32,   // USB Status
    pub pagesize: u32, // Page Size
    pub reserved0: [u32; 2],

    pub dnctrl: u32, // Device Notification Control
    pub crcr: u64,   // Command Ring Control
    pub reserved1: [u32; 4],

    pub dcbaap: u64, // Device Context Base Address Array Pointer
    pub config: u32, // Configure
    pub reserved2: [u32; 49],
    // Port Register Set follows dynamically (MAXPORTS)
}

/* Compile-time size check */
const _: () = assert!(size_of::<XhciOperationalRegisters>() == 256);

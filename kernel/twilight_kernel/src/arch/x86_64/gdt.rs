use alloc::alloc::alloc_zeroed;
use core::alloc::Layout;
use core::arch::asm;
use core::ptr;
use core::ptr::addr_of;
use x86_64::VirtAddr;

const STACK_SIZE: usize = 1024 * 4 * 16;
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PAGE_FAULT_IST: u16 = 1;
pub const GENERAL_PROTECTION_FAULT_IST: u16 = 2;

const BOOT_GDT_ENTRY_COUNT: usize = 4;
const GDT_ENTRY_COUNT: usize = 10;

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone)]
    struct GdtEntryFlags: u8 {
        const PROTECTED_MODE = 1 << 6;
        const LONG_MODE = 1 << 5;
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum PrivilegeLevel {
    Ring0 = 0,
    Ring3 = 3,
}

impl PrivilegeLevel {
    pub fn is_user(&self) -> bool {
        matches!(self, Self::Ring3)
    }
}

struct GdtAccessFlags;

impl GdtAccessFlags {
    const EXECUTABLE: u8 = 1 << 3;
    const NULL: u8 = 0;
    const PRESENT: u8 = 1 << 7;
    const PRIVILEGE: u8 = 1 << 1;
    const RING_0: u8 = 0 << 5;
    const RING_3: u8 = 3 << 5;
    const SYSTEM: u8 = 1 << 4;
    const TSS_AVAIL: u8 = 9;
}

static mut BOOT_GDT: [GdtEntry; BOOT_GDT_ENTRY_COUNT] = [
    // GDT null descriptor.
    GdtEntry::NULL,
    // GDT kernel code descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_0
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::EXECUTABLE
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT kernel data descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_0
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT kernel TLS descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_0
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
];

static GDT: [GdtEntry; GDT_ENTRY_COUNT] = [
    // GDT null descriptor.
    GdtEntry::NULL,
    // GDT kernel code descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_0
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::EXECUTABLE
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT kernel data descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_0
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT kernel TLS descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_0
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT user data descriptor. (used by SYSCALL)
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_3
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT user code descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_3
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::EXECUTABLE
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT user data descriptor. (used by SYSENTER)
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_3
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT user TLS descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT
            | GdtAccessFlags::RING_3
            | GdtAccessFlags::SYSTEM
            | GdtAccessFlags::PRIVILEGE,
        GdtEntryFlags::LONG_MODE,
    ),
    // GDT TSS descriptor.
    GdtEntry::new(
        GdtAccessFlags::PRESENT | GdtAccessFlags::RING_3 | GdtAccessFlags::TSS_AVAIL,
        GdtEntryFlags::empty(),
    ),
    // GDT null descriptor as the TSS should be 16 bytes long
    // and twice the normal size.
    GdtEntry::NULL,
];

#[repr(C, packed)]
pub struct Tss {
    reserved: u32, // offset 0x00

    /// The full 64-bit canonical forms of the stack pointers (RSP) for
    /// privilege levels 0-2.
    pub rsp: [u64; 3], // offset 0x04
    pub reserved2: u64, // offset 0x1C

    /// The full 64-bit canonical forms of the interrupt stack table
    /// (IST) pointers.
    pub ist: [u64; 7], // offset 0x24
    reserved3: u64, // offset 0x5c
    reserved4: u16, // offset 0x64

    /// The 16-bit offset to the I/O permission bit map from the 64-bit
    /// TSS base.
    pub iomap_base: u16, // offset 0x66
}

impl Tss {
    pub fn new() -> Self {
        Self {
            reserved: 0,
            rsp: [0; 3],
            reserved2: 0,
            ist: [0; 7],
            reserved3: 0,
            reserved4: 0,
            iomap_base: 0,
        }
    }
}

pub static mut TSS: Tss = {
    let tss = Tss {
        reserved: 0,
        rsp: [0; 3],
        reserved2: 0,
        ist: [0; 7],
        reserved3: 0,
        reserved4: 0,
        iomap_base: 0,
    };
    tss
};

#[repr(C, packed)]
struct GdtDescriptor {
    /// The size of the table subtracted by 1.
    /// The size of the table is subtracted by 1 as the maximum value
    /// of `size` is 65535, while the GDT can be up to 65536 bytes.
    size: u16,
    /// The linear address of the table.
    offset: u64,
}

impl GdtDescriptor {
    /// Create a new GDT descriptor.
    #[inline]
    pub const fn new(size: u16, offset: u64) -> Self {
        Self { size, offset }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(super) struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_middle: u8,
    access_byte: u8,
    /// The limit high and the flags.
    ///
    /// **Note**: Four bits of the variable is the limit and rest four bits of the
    /// variable are the flags.
    limit_hi_flags: u8,
    base_hi: u8,
}

impl GdtEntry {
    const NULL: Self = Self::new(GdtAccessFlags::NULL, GdtEntryFlags::empty());

    const fn new(access_flags: u8, entry_flags: GdtEntryFlags) -> Self {
        Self {
            limit_low: 0x00,
            base_low: 0x00,
            base_middle: 0x00,
            access_byte: access_flags,
            limit_hi_flags: entry_flags.bits() & 0xF0,
            base_hi: 0x00,
        }
    }

    fn set_offset(&mut self, offset: u32) {
        self.base_low = offset as u16;
        self.base_middle = (offset >> 16) as u8;
        self.base_hi = (offset >> 24) as u8;
    }

    fn set_limit(&mut self, limit: u32) {
        self.limit_low = limit as u16;
        self.limit_hi_flags = self.limit_hi_flags & 0xF0 | ((limit >> 16) as u8) & 0x0F;
    }

    fn set_raw<T>(&mut self, value: T) {
        unsafe {
            *(ptr::addr_of_mut!(*self).cast::<T>()) = value;
        }
    }
}

pub struct GdtEntryIndex;

#[rustfmt::skip]
impl GdtEntryIndex {
    pub const KERNEL_CODE: u16 = 1;
    pub const KERNEL_DATA: u16 = 2;
    pub const KERNEL_TLS: u16 = 3;
    pub const USER_DATA: u16 = 4;
    pub const USER_CODE: u16 = 5;
    pub const TSS: u16 = 8;
    pub const TSS_HI: u16 = 9;
}

#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct SegmentSelector(u16);

impl SegmentSelector {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn new(index: u16, privilege_level: PrivilegeLevel) -> Self {
        Self(index << 3 | (privilege_level as u16))
    }

    pub const fn bits(&self) -> u16 {
        self.0
    }

    pub const fn from_bits(value: u16) -> Self {
        Self(value)
    }

    pub const fn privilege_level(&self) -> PrivilegeLevel {
        match self.bits() & 0b11 {
            0 => PrivilegeLevel::Ring0,
            3 => PrivilegeLevel::Ring3,
            _ => unreachable!(),
        }
    }
}

pub fn init() {
    let gdt_descriptor = {
        GdtDescriptor::new(
            (size_of::<[GdtEntry; BOOT_GDT_ENTRY_COUNT]>() - 1) as u16,
            addr_of!(BOOT_GDT).addr() as u64,
        )
    };

    load_gdt(&gdt_descriptor);

    // Load the GDT segments.
    load_cs(SegmentSelector::new(
        GdtEntryIndex::KERNEL_CODE,
        PrivilegeLevel::Ring0,
    ));

    load_ds(SegmentSelector::new(
        GdtEntryIndex::KERNEL_DATA,
        PrivilegeLevel::Ring0,
    ));

    load_es(SegmentSelector::new(
        GdtEntryIndex::KERNEL_DATA,
        PrivilegeLevel::Ring0,
    ));

    load_fs(SegmentSelector::new(
        GdtEntryIndex::KERNEL_DATA,
        PrivilegeLevel::Ring0,
    ));

    load_gs(SegmentSelector::new(
        GdtEntryIndex::KERNEL_TLS,
        PrivilegeLevel::Ring0,
    ));

    load_ss(SegmentSelector::new(
        GdtEntryIndex::KERNEL_DATA,
        PrivilegeLevel::Ring0,
    ));
}

static STK: [u8; 4096 * 16] = [0; 4096 * 16];

pub const USER_SS: SegmentSelector =
    SegmentSelector::new(GdtEntryIndex::USER_DATA, PrivilegeLevel::Ring3);

pub const USER_CS: SegmentSelector =
    SegmentSelector::new(GdtEntryIndex::USER_CODE, PrivilegeLevel::Ring3);

pub fn init_after_boot() {
    let gdt = unsafe {
        let gdt_ent_size = size_of::<GdtEntry>();
        let gdt_ent_align = align_of::<GdtEntry>();

        let gdt_size = gdt_ent_size * GDT_ENTRY_COUNT;
        let layout = Layout::from_size_align_unchecked(gdt_size, gdt_ent_align);

        let ptr = alloc_zeroed(layout).cast::<GdtEntry>();
        core::slice::from_raw_parts_mut::<GdtEntry>(ptr, GDT_ENTRY_COUNT)
    };

    // Copy over the GDT template:
    gdt.copy_from_slice(&GDT);

    unsafe {
        TSS.ist[DOUBLE_FAULT_IST_INDEX as usize] = {
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            (VirtAddr::from_ptr(addr_of!(STACK)) + STACK_SIZE as u64).as_u64()
        };
        TSS.ist[PAGE_FAULT_IST as usize] = {
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            (VirtAddr::from_ptr(addr_of!(STACK)) + STACK_SIZE as u64).as_u64()
        };
        TSS.ist[GENERAL_PROTECTION_FAULT_IST as usize] = {
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            (VirtAddr::from_ptr(addr_of!(STACK)) + STACK_SIZE as u64).as_u64()
        };
    }

    unsafe {
        #[allow(static_mut_refs)]
        let tss_ptr = &mut TSS;

        gdt[GdtEntryIndex::TSS as usize].set_offset(tss_ptr as *const _ as u32);
        gdt[GdtEntryIndex::TSS as usize].set_limit(size_of::<Tss>() as u32);
        gdt[GdtEntryIndex::TSS_HI as usize].set_raw((tss_ptr as *const _ as u64) >> 32);

        TSS.rsp[0] = STK.as_ptr().offset(4096 * 16) as u64;

        let gdt_descriptor = GdtDescriptor::new(
            (size_of::<[GdtEntry; GDT_ENTRY_COUNT]>() - 1) as u16,
            gdt.as_ptr() as u64,
        );

        load_gdt(&gdt_descriptor);

        // Reload the GDT segments.
        load_cs(SegmentSelector::new(
            GdtEntryIndex::KERNEL_CODE,
            PrivilegeLevel::Ring0,
        ));
        load_ds(SegmentSelector::new(
            GdtEntryIndex::KERNEL_DATA,
            PrivilegeLevel::Ring0,
        ));
        load_es(SegmentSelector::new(
            GdtEntryIndex::KERNEL_DATA,
            PrivilegeLevel::Ring0,
        ));
        load_ss(SegmentSelector::new(
            GdtEntryIndex::KERNEL_DATA,
            PrivilegeLevel::Ring0,
        ));

        // Load the Task State Segment.
        load_tss(SegmentSelector::new(
            GdtEntryIndex::TSS,
            PrivilegeLevel::Ring0,
        ));
    }

    // // Now we update the per-cpu storage to store a reference
    // // to the per-cpu GDT.
    // tls::get_percpu().gdt = gdt;
}

#[inline(always)]
#[allow(binary_asm_labels)]
fn load_cs(selector: SegmentSelector) {
    // NOTE: We cannot directly move into CS since x86 requires the IP
    // and CS set at the same time. To do this, we need push the new segment
    // selector and return value onto the stack and far return to reload CS and
    // continue execution.
    //
    // We also cannot use a far call or a far jump since we would only be
    // able to jump to 32-bit instruction pointers. Only Intel supports for
    // 64-bit far calls/jumps in long-mode, AMD does not.
    unsafe {
        asm!(
        "push {selector}",
        "lea {tmp}, [1f + rip]",
        "push {tmp}",
        "retfq",
        "1:",
        selector = in(reg) u64::from(selector.bits()),
        tmp = lateout(reg) _,
        );
    }
}

#[inline(always)]
fn load_ds(selector: SegmentSelector) {
    unsafe { asm!("mov ds, {0:x}", in(reg) selector.bits(), options(nomem, nostack)) }
}

#[inline(always)]
fn load_es(selector: SegmentSelector) {
    unsafe { asm!("mov es, {0:x}", in(reg) selector.bits(), options(nomem, nostack)) }
}

#[inline(always)]
fn load_fs(selector: SegmentSelector) {
    unsafe { asm!("mov fs, {0:x}", in(reg) selector.bits(), options(nomem, nostack)) }
}

#[inline(always)]
fn load_gs(selector: SegmentSelector) {
    unsafe { asm!("mov gs, {0:x}", in(reg) selector.bits(), options(nomem, nostack)) }
}

#[inline(always)]
fn load_ss(selector: SegmentSelector) {
    unsafe { asm!("mov ss, {0:x}", in(reg) selector.bits(), options(nomem, nostack)) }
}

#[inline(always)]
fn load_tss(selector: SegmentSelector) {
    unsafe {
        asm!("ltr {0:x}", in(reg) selector.bits(), options(nostack, nomem));
    }
}

#[inline(always)]
fn load_gdt(gdt_descriptor: &GdtDescriptor) {
    unsafe {
        asm!("lgdt [{}]", in(reg) gdt_descriptor, options(nostack));
    }
}

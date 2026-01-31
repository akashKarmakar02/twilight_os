#![allow(dead_code)]
/* =========================================================
   Configuration Definitions
========================================================= */

pub const XHCI_COMMAND_RING_TRB_COUNT: usize  = 256;
pub const XHCI_EVENT_RING_TRB_COUNT: usize    = 256;
pub const XHCI_TRANSFER_RING_TRB_COUNT: usize = 256;

/* =========================================================
   USBCMD Register Bits
========================================================= */

pub const XHCI_USBCMD_RUN_STOP: u32                 = 1 << 0;
pub const XHCI_USBCMD_HCRESET: u32                  = 1 << 1;
pub const XHCI_USBCMD_INTERRUPTER_ENABLE: u32       = 1 << 2;
pub const XHCI_USBCMD_HOSTSYS_ERROR_ENABLE: u32     = 1 << 3;
pub const XHCI_USBCMD_LIGHT_HCRESET: u32            = 1 << 7;
pub const XHCI_USBCMD_CSS: u32                      = 1 << 8;
pub const XHCI_USBCMD_CRS: u32                      = 1 << 9;
pub const XHCI_USBCMD_EWE: u32                      = 1 << 10;

/* =========================================================
   USBSTS Register Bits
========================================================= */

pub const XHCI_USBSTS_HCH: u32  = 1 << 0;
pub const XHCI_USBSTS_HSE: u32  = 1 << 2;
pub const XHCI_USBSTS_EINT: u32 = 1 << 3;
pub const XHCI_USBSTS_PCD: u32  = 1 << 4;
pub const XHCI_USBSTS_SSS: u32  = 1 << 8;
pub const XHCI_USBSTS_RSS: u32  = 1 << 9;
pub const XHCI_USBSTS_SRE: u32  = 1 << 10;
pub const XHCI_USBSTS_CNR: u32  = 1 << 11;
pub const XHCI_USBSTS_HCE: u32  = 1 << 12;

/* =========================================================
   Capability Register Structure
========================================================= */

#[repr(C)]
pub struct XhciCapabilityRegs {
    pub hcsparams1: u32,
    pub hcsparams2: u32,
    pub hcsparams3: u32,
    pub hccparams1: u32,
    pub hccparams2: u32,
}

/* =========================================================
   HCSPARAMS1 helpers
========================================================= */

#[inline(always)]
pub fn xhci_max_device_slots(r: &XhciCapabilityRegs) -> u8 {
    (r.hcsparams1 & 0xFF) as u8
}

#[inline(always)]
pub fn xhci_max_interrupters(r: &XhciCapabilityRegs) -> u16 {
    ((r.hcsparams1 >> 8) & 0x7FF) as u16
}

#[inline(always)]
pub fn xhci_max_ports(r: &XhciCapabilityRegs) -> u8 {
    ((r.hcsparams1 >> 24) & 0xFF) as u8
}

/* =========================================================
   HCSPARAMS2 helpers
========================================================= */

#[inline(always)] pub fn xhci_ist(r: &XhciCapabilityRegs) -> u8 { (r.hcsparams2 & 0xF) as u8 }
#[inline(always)] pub fn xhci_erst_max(r: &XhciCapabilityRegs) -> u8 { ((r.hcsparams2 >> 4) & 0xF) as u8 }
#[inline(always)] pub fn xhci_max_scratchpad_bufs_hi(r: &XhciCapabilityRegs) -> u8 { ((r.hcsparams2 >> 21) & 0x1F) as u8 }
#[inline(always)] pub fn xhci_spr(r: &XhciCapabilityRegs) -> bool { ((r.hcsparams2 >> 26) & 1) != 0 }
#[inline(always)] pub fn xhci_max_scratchpad_bufs_lo(r: &XhciCapabilityRegs) -> u8 { ((r.hcsparams2 >> 27) & 0x1F) as u8 }

#[inline(always)]
pub fn xhci_max_scratchpad_buffers(r: &XhciCapabilityRegs) -> u16 {
    ((xhci_max_scratchpad_bufs_hi(r) as u16) << 5)
        | (xhci_max_scratchpad_bufs_lo(r) as u16)
}

/* =========================================================
   HCSPARAMS3 helpers
========================================================= */

#[inline(always)] pub fn xhci_u1_exit_latency(r: &XhciCapabilityRegs) -> u8 { (r.hcsparams3 & 0xFF) as u8 }
#[inline(always)] pub fn xhci_u2_exit_latency(r: &XhciCapabilityRegs) -> u16 { ((r.hcsparams3 >> 16) & 0xFFFF) as u16 }

/* =========================================================
   HCCPARAMS1 helpers
========================================================= */

#[inline(always)] pub fn xhci_ac64(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & 1 != 0 }
#[inline(always)] pub fn xhci_bnc(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 1) != 0 }
#[inline(always)] pub fn xhci_csz(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 2) != 0 }
#[inline(always)] pub fn xhci_ppc(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 3) != 0 }
#[inline(always)] pub fn xhci_pind(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 4) != 0 }
#[inline(always)] pub fn xhci_lhrc(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 5) != 0 }
#[inline(always)] pub fn xhci_ltc(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 6) != 0 }
#[inline(always)] pub fn xhci_nss(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 7) != 0 }
#[inline(always)] pub fn xhci_pae(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 8) != 0 }
#[inline(always)] pub fn xhci_spc(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 9) != 0 }
#[inline(always)] pub fn xhci_sec(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 10) != 0 }
#[inline(always)] pub fn xhci_cfc(r: &XhciCapabilityRegs) -> bool { r.hccparams1 & (1 << 11) != 0 }

#[inline(always)]
pub fn xhci_max_psa_size(r: &XhciCapabilityRegs) -> u8 {
    ((r.hccparams1 >> 12) & 0xF) as u8
}

#[inline(always)]
pub fn xhci_xecp(r: &XhciCapabilityRegs) -> u16 {
    ((r.hccparams1 >> 16) & 0xFFFF) as u16
}

/* =========================================================
   HCCPARAMS2 helpers
========================================================= */

#[inline(always)] pub fn xhci_u3c(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & 1 != 0 }
#[inline(always)] pub fn xhci_cmc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 1) != 0 }
#[inline(always)] pub fn xhci_fsc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 2) != 0 }
#[inline(always)] pub fn xhci_ctc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 3) != 0 }
#[inline(always)] pub fn xhci_lec(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 4) != 0 }
#[inline(always)] pub fn xhci_cic(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 5) != 0 }
#[inline(always)] pub fn xhci_etc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 6) != 0 }
#[inline(always)] pub fn xhci_etc_tsc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 7) != 0 }
#[inline(always)] pub fn xhci_gsc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 8) != 0 }
#[inline(always)] pub fn xhci_vtc(r: &XhciCapabilityRegs) -> bool { r.hccparams2 & (1 << 9) != 0 }

/* =========================================================
   CONFIG Register Helpers
========================================================= */

#[inline(always)] pub fn xhci_max_slots_en(config: u32) -> u8 { (config & 0xFF) as u8 }

#[inline(always)]
pub fn xhci_set_max_slots_en(config: u32, slots: u8) -> u32 {
    (config & !0xFF) | (slots as u32)
}

#[inline(always)] pub fn xhci_u3_entry_enable(config: u32) -> bool { ((config >> 8) & 1) != 0 }
#[inline(always)] pub fn xhci_config_info_enable(config: u32) -> bool { ((config >> 9) & 1) != 0 }

/* =========================================================
   TRB Types
========================================================= */

#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TrbType {
    Reserved = 0,
    Normal = 1,
    SetupStage = 2,
    DataStage = 3,
    StatusStage = 4,
    Isoch = 5,
    Link = 6,
    EventData = 7,
    Noop = 8,
    EnableSlotCmd = 9,
    DisableSlotCmd = 10,
    AddressDeviceCmd = 11,
    ConfigureEndpointCmd = 12,
    EvaluateContextCmd = 13,
    ResetEndpointCmd = 14,
    StopEndpointCmd = 15,
    SetTrDequeuePtrCmd = 16,
    ResetDeviceCmd = 17,
    ForceEventCmd = 18,
    NegotiateBandwidthCmd = 19,
    SetLatencyToleranceCmd = 20,
    GetPortBandwidthCmd = 21,
    ForceHeaderCmd = 22,
    NoopCmd = 23,
}




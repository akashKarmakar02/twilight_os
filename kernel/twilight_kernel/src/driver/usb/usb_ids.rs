// Simple USB Vendor/Product ID lookup table.
// In a real OS, this would be a much larger database or loaded from userspace.
// For now, it's a hardcoded helper to resolve devices that don't nicely report their strings.

pub fn lookup(vid: u16, pid: u16) -> Option<(&'static str, &'static str)> {
    match (vid, pid) {
        (0x1bcf, 0x08a0) => Some((
            "Sunplus Innovation Technology Inc.",
            "Gaming mouse [Philips SPK9304]",
        )),
        (0x8086, 0x7020) => Some(("Intel Corp.", "UHCI Controller")),

        // QEMU / Virtualization
        (0x0627, 0x0001) => Some(("QEMU", "USB Tablet")),
        (0x0409, 0x005a) => Some(("NEC Corp.", "HighSpeed Hub")), // Common in QEMU XHCI

        // Linux Foundation Root Hubs (Common in all Linux-based guests/hosts)
        (0x1d6b, 0x0001) => Some(("Linux Foundation", "1.1 Root Hub")),
        (0x1d6b, 0x0002) => Some(("Linux Foundation", "2.0 Root Hub")),
        (0x1d6b, 0x0003) => Some(("Linux Foundation", "3.0 Root Hub")),

        // VMware
        (0x0e0f, 0x0002) => Some(("VMware, Inc.", "Virtual USB Hub")),
        (0x0e0f, 0x0003) => Some(("VMware, Inc.", "Virtual USB Mouse")),
        (0x0e0f, 0x0008) => Some(("VMware, Inc.", "Virtual USB Keyboard")),

        // Common Realtek (from your logs)
        (0x0bda, 0xc829) => Some(("Realtek Semiconductor Corp.", "Bluetooth Radio")),
        (0x0bda, 0x5522) => Some(("Realtek Semiconductor Corp.", "Integrated Webcam HD")),

        _ => None,
    }
}

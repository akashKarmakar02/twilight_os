use x86_64::instructions::port::Port;

pub fn poweroff() {
    let mut port = Port::new(0x604);
    unsafe {
        port.write(0x2000u16);
    }
}

pub fn restart() {
    let mut port = Port::new(0x64);
    unsafe {
        port.write(0xfe_u8);
    }
}

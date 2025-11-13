use x86_64::instructions::port::Port;

pub struct Uart {
    port: u16,
}

#[allow(static_mut_refs)]
pub static mut UART: Option<Uart> = None;

impl Uart {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub fn init(&self) {
        let mut port: Port<u8> = Port::new(self.port + 1);

        unsafe {
            port.write(0x00u8); //disable interrupts
        }

        let mut port: Port<u8> = Port::new(self.port + 3);

        unsafe {
            port.write(0x80u8); // Enable DLAB (set baud rate divisor)
        }

        let mut port: Port<u8> = Port::new(self.port);

        unsafe {
            port.write(0x03u8); // Set divisor to 3 (lo byte) 38400 baud
        }

        let mut port: Port<u8> = Port::new(self.port + 1);

        unsafe {
            port.write(0x00u8); // High byte for divisor
        }

        let mut port: Port<u8> = Port::new(self.port + 3);

        unsafe {
            port.write(0x03u8); // 8 bits, no parity, one stop bit
        }

        let mut port: Port<u8> = Port::new(self.port + 2);

        unsafe {
            port.write(0xC7u8); // Enable FIFO, clear them, with 14-byte threshold
        }

        let mut port: Port<u8> = Port::new(self.port + 4);

        unsafe {
            port.write(0x0Bu8); // IRQs enabled, RTS/DSR set
        }
    }

    fn is_transmit_empty(&self) -> bool {
        unsafe {
            let mut port = Port::new(self.port + 5);
            let status: u8 = port.read();
            status & 0x20 != 0
        }
    }

    pub fn send(&self, data: u8) {
        while !self.is_transmit_empty() {}
        unsafe {
            let mut port = Port::new(self.port);
            port.write(data);
        }
    }

    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' | b'\r' => self.send(byte),
                _ => self.send(0xfe),
            }
        }
    }
}

pub fn init() {
    unsafe {
        let uart = Uart::new(0x3f8);
        uart.init();

        UART = Some(uart);
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::driver::uart::_print(format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    unsafe {
        #[allow(static_mut_refs)]
        if let Some(uart) = &UART {
            use core::fmt::Write;
            let _ = SerialWriter(uart).write_fmt(args);
        }
    }
}

#[macro_export]
macro_rules! serial_println {
    ($($arg:tt)*) => {
        $crate::serial_print!($($arg)*);
        crate::serial_print!("\n");
    };
}

use core::fmt::{self, Write};

pub struct SerialWriter<'a>(pub &'a Uart);

impl<'a> Write for SerialWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s);
        Ok(())
    }
}

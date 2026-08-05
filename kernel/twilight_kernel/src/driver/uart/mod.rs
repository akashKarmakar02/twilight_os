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

        // Enable received-data-available interrupt so the host can type into
        // the serial console. Without this, bytes sent over the serial FIFO
        // (e.g. by tools/time-regression/run.sh) never reach the TTY.
        let mut port: Port<u8> = Port::new(self.port + 1);
        unsafe {
            port.write(0x01u8); // IER bit 0: received-data interrupt
        }
    }

    fn is_transmit_empty(&self) -> bool {
        unsafe {
            let mut port = Port::new(self.port + 5);
            let status: u8 = port.read();
            status & 0x20 != 0
        }
    }

    /// Line Status Register (offset 5).
    fn line_status(&self) -> u8 {
        unsafe {
            let mut port = Port::new(self.port + 5);
            port.read()
        }
    }

    /// Read one byte from the receive register. Returns None if no data is
    /// available. The Data Ready bit is LSR bit 0.
    pub fn receive(&self) -> Option<u8> {
        if self.line_status() & 0x01 == 0 {
            return None;
        }
        unsafe {
            let mut port = Port::new(self.port);
            Some(port.read())
        }
    }

    pub fn send(&self, data: u8) {
        while !self.is_transmit_empty() {}
        unsafe {
            let mut port = Port::new(self.port);
            port.write(data);
        }
    }

    /// Send a byte without filtering. Used by the console mirror path.
    pub fn send_raw(&self, data: u8) {
        self.send(data);
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

/// Send a single byte to the serial port without filtering. Used by the
/// console mirror path to forward process output to the serial console.
pub fn send_byte(b: u8) {
    unsafe {
        #[allow(static_mut_refs)]
        if let Some(uart) = &UART {
            uart.send_raw(b);
        }
    }
}

/// Register the COM1 receive-interrupt handler. Must be called *after* the IDT
/// and PICs are initialized, since `register_irq_handler` unmasks the IRQ in
/// the PIC and installs into the IDT's handler table.
pub fn init_input_irq() {
    if crate::arch::x86_64::idt::register_irq_handler(4, handle_serial_input).is_ok() {
        crate::serial_println!("[uart] serial input enabled on COM1 (IRQ 4)");
    } else {
        crate::serial_println!("[uart] warning: could not register COM1 IRQ handler");
    }
}

/// IRQ 4 handler: drain the UART receive FIFO into the console TTY.
fn handle_serial_input() {
    unsafe {
        #[allow(static_mut_refs)]
        if let Some(uart) = &UART {
            while let Some(byte) = uart.receive() {
                // Map CR to LF so a host sending "\r\n" or "\r" produces a
                // single newline in the TTY input buffer.
                let c = if byte == b'\r' { b'\n' } else { byte };
                crate::sys::console::put_char_in_tty(c);
            }
        }
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

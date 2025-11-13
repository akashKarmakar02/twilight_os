use crate::driver::timer::wait;
use crate::log;
use x86_64::instructions::port::Port;

#[allow(dead_code)]
#[derive(Clone)]
pub struct UHci {
    usb_cmd: Port<u16>,
    usb_status: Port<u16>,
    usb_interrupt: Port<u16>,
    usb_frame_no: Port<u16>,
    framelist_addr: Port<u32>,
    sof_modifier: Port<u16>,
    ctrl1: Port<u16>,
    ctrl2: Port<u16>,
}

#[allow(dead_code)]
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UhciTD {
    pub link_ptr: u32,
    pub ctrl_status: u32,
    pub token: u32,
    pub buffer_ptr: u32,
}

#[allow(dead_code)]
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UhciQH {
    pub head_link: u32,
    pub element_link: u32,
}

impl UHci {
    pub fn new(io_base: u16) -> Self {
        Self {
            usb_cmd: Port::new(io_base + 0x00),
            usb_status: Port::new(io_base + 0x02),
            usb_interrupt: Port::new(io_base + 0x04),
            usb_frame_no: Port::new(io_base + 0x06),
            framelist_addr: Port::new(io_base + 0x08),
            sof_modifier: Port::new(io_base + 0x0c),
            ctrl1: Port::new(io_base + 0x10),
            ctrl2: Port::new(io_base + 0x12),
        }
    }

    pub fn list(&mut self) {
        // reset controller
        unsafe {
            self.usb_cmd.write(0x02);
        }
        wait(1000 * 5);

        // clear status
        unsafe {
            self.usb_status.write(0x00);
        }

        // allocate frame list
        let frame_list_phys_addr = 0x1000;
        unsafe {
            self.framelist_addr.write(frame_list_phys_addr);
        }

        // start controller
        unsafe {
            self.usb_cmd.write(0x01);
        }

        let portctrl1 = unsafe { self.ctrl1.read() };
        let portctrl2 = unsafe { self.ctrl2.read() };

        if portctrl1 & 0x01 != 0 {
            log!("USB Device found on controller1: {:x}", portctrl1);
            self.reset_port(1);
        }

        if portctrl2 & 0x01 != 0 {
            log!("USB Device found on controller2: {:x}", portctrl2);
            self.reset_port(2);
        }
    }

    fn reset_port(&mut self, port: u8) {
        let mut ctrl = if port == 1 {
            self.ctrl1.clone()
        } else {
            self.ctrl2.clone()
        };

        unsafe {
            let mut val = ctrl.read();
            val |= 0x04;
            ctrl.write(val);
            wait(1000 * 5);
            val &= !0x04;
            ctrl.write(val);
            val |= 0x01;
            ctrl.write(val);
        }
    }
}

use crate::driver::disk::{BlockDeviceIO, USB_BLOCK_DEVICE};
use crate::log;
use alloc::boxed::Box;

pub struct UsbMscBlockDevice {
    controller_id: usize,
    msc_index: usize,
    block_size: u32,
    block_count: u32,
}

impl UsbMscBlockDevice {
    pub fn new(controller_id: usize, msc_index: usize, block_size: u32, block_count: u32) -> Self {
        Self {
            controller_id,
            msc_index,
            block_size,
            block_count,
        }
    }
}

impl BlockDeviceIO for UsbMscBlockDevice {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.len() != self.block_size as usize {
            return Err(());
        }
        #[allow(static_mut_refs)]
        if unsafe { super::XHCI_DEVICES.get().is_some() } {
            let xhci =  unsafe { super::XHCI_DEVICES.get_mut_unchecked() };
            let hc = xhci.get_mut(self.controller_id).ok_or(())?;
            hc.msc_read(self.msc_index, lba, buf).map_err(|_| ())
        } else {
            Ok(())
        }
    }

    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        if buf.len() != self.block_size as usize {
            return Err(());
        }
        #[allow(static_mut_refs)]
        if unsafe { !super::XHCI_DEVICES.get().is_some() } {
            let mut xhci = unsafe { super::XHCI_DEVICES.get_mut_unchecked() };
            let hc = xhci.get_mut(self.controller_id).ok_or(())?;
            hc.msc_write(self.msc_index, lba, buf).map_err(|_| ())
        }else {
            Ok(())
        }
    }

    fn block_size(&self) -> usize {
        self.block_size as usize
    }

    fn block_count(&self) -> usize {
        self.block_count as usize
    }
}

pub fn register_usb_msc_block_device(
    controller_id: usize,
    msc_index: usize,
    block_size: u32,
    block_count: u32,
) {
    let dev = Box::leak(Box::new(UsbMscBlockDevice::new(
        controller_id,
        msc_index,
        block_size,
        block_count,
    )));

    #[allow(static_mut_refs)]
    unsafe {
        if USB_BLOCK_DEVICE.is_some() {
            log!("USB MSC: replacing external block device");
        }
        USB_BLOCK_DEVICE = Some(dev);
    }
}

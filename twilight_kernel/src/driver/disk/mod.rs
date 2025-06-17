use crate::sys::fs::minixfs::MinixFs;
use spin::Once;

pub mod ata;

#[allow(static_mut_refs)]
pub static mut DISK: Once<&mut dyn BlockDevice> = Once::new();

pub static mut DISK_FS: Once<MinixFs> = Once::new();

pub trait BlockDevice {
    fn read_block(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), &'static str>;

    fn write_block(&mut self, lba: u64, buffer: &[u8]) -> Result<(), &'static str>;

    fn sector_count(&mut self) -> u64;

    fn sector_size(&mut self) -> u64;

    fn send_command(&mut self, command: u32, buffer: &mut [u8]) -> Result<(), &'static str>;
}

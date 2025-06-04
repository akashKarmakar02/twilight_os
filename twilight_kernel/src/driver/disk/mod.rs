use crate::driver::disk::ata::Ata;
use crate::println;
use alloc::boxed::Box;
use spin::Once;

pub mod ata;


#[allow(static_mut_refs)]
pub static mut DISK: Once<&mut dyn BlockDevice> = Once::new();

pub trait BlockDevice {
    fn read_block(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), &'static str>;

    fn write_block(&mut self, lba: u64, buffer: &[u8]) -> Result<(), &'static str>;

    fn sector_count(&mut self) -> u64;

    fn sector_size(&mut self) -> u64;

    fn send_command(&mut self, command: u32, buffer: &mut [u8]) -> Result<(), &'static str>;
}


pub fn init() {
    let mut ata = Ata {
        base_port: 0x1F0,
        sector_size: 1024,
        sector_count: 0,
        model: None,
    };

    if ata.detect() {
        println!(
            "ATA: {} {}",
            ata.base_port,
            ata.get_ata_model_name().unwrap()
        );
        
        let static_ata: &'static mut Ata = Box::leak(Box::new(ata));

        #[allow(static_mut_refs)]
        unsafe {
            DISK.call_once(|| {
                static_ata as &mut dyn BlockDevice
            });
        }
        
        #[allow(static_mut_refs)]
        let d = unsafe { DISK.get_mut().unwrap() };

        match crate::fs::minixfs::read_superblock(*d) {
            Ok(_) => {
                println!("Minix FS found on disk");
            }
            Err(_) => {
                println!("Not FS found on disk");
            }
        };
    }
}
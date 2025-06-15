use crate::driver::disk::ata::Ata;
use crate::println;
use alloc::boxed::Box;
use spin::Once;
use crate::fs::minixfs::MinixFs;

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


pub fn init() {
    let mut ata = Ata {
        base_port: 0x1F0,
        sector_size: 1024,
        sector_count: 0,
        model: None,
    };

    let time = crate::driver::timer::pit::uptime();

    if ata.detect() {
        println!(
            "\x1b[93m[{:.6}]\x1b[0m ATA: {} bytes {}",
            time,
            ata.sector_size * ata.sector_count as usize,
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
            Ok(sb) => {
                println!("Minix FS found on disk");
                #[allow(static_mut_refs)]
                unsafe {
                    DISK_FS.call_once(|| {
                        MinixFs {
                            device: *d,
                            superblock: sb,
                        }
                    });
                }
            }
            Err(_) => {
                println!("Not FS found on disk");
            }
        };
    }
}
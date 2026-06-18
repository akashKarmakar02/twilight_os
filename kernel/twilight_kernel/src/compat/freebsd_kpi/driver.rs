use super::device::Device;

pub const BUS_PROBE_DEFAULT: i32 = 0;
pub const ENXIO: i32 = 6;
pub const ENOMEM: i32 = 12;
pub const EINVAL: i32 = 22;

pub trait FreeBsdPciDriver {
    fn probe(&mut self, device: &mut Device) -> i32;
    fn attach(&mut self, device: &mut Device) -> i32;
    fn detach(&mut self, device: &mut Device) -> i32;
}

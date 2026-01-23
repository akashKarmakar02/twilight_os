use crate::driver::timer::pit;
use crate::sys::fs::vfs::BlockDev;
use crate::sys::fs::vfs::VfsNodeOps;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use core::cmp::min;
use raw_cpuid::CpuId;
use twilight_common::syscall::types::ENOTTY;

pub const IOCTL_PROC_GET_SIZE: u64 = 0x5457_0001;

fn read_from_string(offset: usize, buf: &mut [u8], s: &str) -> Result<usize, ()> {
    let bytes = s.as_bytes();
    if offset >= bytes.len() {
        return Ok(0);
    }
    let n = min(buf.len(), bytes.len() - offset);
    buf[..n].copy_from_slice(&bytes[offset..offset + n]);
    Ok(n)
}

fn cpu_count() -> usize {
    crate::driver::cpu::cpu_count()
}

fn cpu_mhz() -> u64 {
    // driver::timer::tsc_frequency() currently behaves like "cycles per microsecond".
    // That number is MHz for an invariant TSC.
    let mhz = crate::driver::timer::tsc_frequency();
    if mhz == 0 { 0 } else { mhz }
}

fn build_cpuinfo() -> String {
    let cpuid = CpuId::new();
    let vendor = cpuid
        .get_vendor_info()
        .map(|v| v.as_str().to_string())
        .unwrap_or_else(|| "Unknown".into());
    let brand = cpuid
        .get_processor_brand_string()
        .map(|b| b.as_str().trim().to_string())
        .unwrap_or_else(|| "Unknown CPU".into());

    let (family, model, stepping) = cpuid
        .get_feature_info()
        .map(|f| (f.family_id() as u32, f.model_id() as u32, f.stepping_id() as u32))
        .unwrap_or((0, 0, 0));

    let mhz = cpu_mhz();
    let nproc = cpu_count().max(1);
    let mut out = String::new();

    for i in 0..nproc {
        out.push_str(&format!("processor\t: {}\n", i));
        out.push_str(&format!("vendor_id\t: {}\n", vendor));
        out.push_str("cpu family\t: ");
        out.push_str(&family.to_string());
        out.push('\n');
        out.push_str("model\t\t: ");
        out.push_str(&model.to_string());
        out.push('\n');
        out.push_str("model name\t: ");
        out.push_str(&brand);
        out.push('\n');
        out.push_str("stepping\t: ");
        out.push_str(&stepping.to_string());
        out.push('\n');
        if mhz != 0 {
            out.push_str(&format!("cpu MHz\t\t: {}\n", mhz));
        }
        out.push_str(&format!("cpu cores\t: {}\n", nproc));
        out.push_str(&format!("siblings\t: {}\n", nproc));
        out.push_str("\n");
    }
    out
}

fn build_meminfo() -> String {
    let (total_bytes, free_bytes) = crate::sys::memory::mem_stats_bytes();
    let total_kb = total_bytes / 1024;
    let free_kb = free_bytes / 1024;

    // Minimal Linux-style /proc/meminfo keys commonly parsed by fetch tools.
    let mut out = String::new();
    out.push_str(&format!("MemTotal:\t{} kB\n", total_kb));
    out.push_str(&format!("MemFree:\t{} kB\n", free_kb));
    out.push_str(&format!("MemAvailable:\t{} kB\n", free_kb));
    out.push_str("Buffers:\t0 kB\n");
    out.push_str("Cached:\t\t0 kB\n");
    out.push_str("SwapCached:\t0 kB\n");
    out.push_str("SwapTotal:\t0 kB\n");
    out.push_str("SwapFree:\t0 kB\n");
    out
}

fn build_uptime() -> String {
    let up = pit::uptime();
    // Linux: "<uptime_seconds> <idle_seconds>\n"
    // We don't track idle time yet -> 0.00.
    format!("{:.2} {:.2}\n", up, 0.00)
}

fn build_version() -> String {
    // Linux-style formatting but with TwilightOS branding.
    // Keep in sync with `uname()` strings.
    "TwilightOS version 0.1.0-testing-build.x86_64 (#1 SMP PREEMPT)\n".into()
}

pub struct CpuInfoNode;
pub struct MemInfoNode;
pub struct UptimeNode;
pub struct VersionNode;

impl VfsNodeOps for CpuInfoNode {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let s = build_cpuinfo();
        read_from_string(offset, buf, &s)
    }
    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }
    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }
    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()> {
        match cmd {
            IOCTL_PROC_GET_SIZE => Ok(build_cpuinfo().len() as i64),
            _ => {
                if arg == 0 {
                    return Ok(-(ENOTTY as i64));
                }
                Ok(-(ENOTTY as i64))
            }
        }
    }
    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

impl VfsNodeOps for MemInfoNode {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let s = build_meminfo();
        read_from_string(offset, buf, &s)
    }
    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }
    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }
    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, _arg: usize) -> Result<i64, ()> {
        match cmd {
            IOCTL_PROC_GET_SIZE => Ok(build_meminfo().len() as i64),
            _ => Ok(-(ENOTTY as i64)),
        }
    }
    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

impl VfsNodeOps for UptimeNode {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let s = build_uptime();
        read_from_string(offset, buf, &s)
    }
    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }
    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }
    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, _arg: usize) -> Result<i64, ()> {
        match cmd {
            IOCTL_PROC_GET_SIZE => Ok(build_uptime().len() as i64),
            _ => Ok(-(ENOTTY as i64)),
        }
    }
    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

impl VfsNodeOps for VersionNode {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let s = build_version();
        read_from_string(offset, buf, &s)
    }
    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }
    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }
    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, _arg: usize) -> Result<i64, ()> {
        match cmd {
            IOCTL_PROC_GET_SIZE => Ok(build_version().len() as i64),
            _ => Ok(-(ENOTTY as i64)),
        }
    }
    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

use crate::driver::disk::BlockDeviceIO;
use crate::sys::fs::twilight_fs::TfsError;
use crate::sys::fs::twilight_fs::inode::Inode;
use crate::sys::proc::Process;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::utils::sync::Mutex;
use crate::utils::sync::RwLock;

pub static mut VFS: RwLock<Vfs> = RwLock::new(Vfs::new());

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    File,
    Dir,
    CharDevice,
    BlockDevice,
    Socket,
}

#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u16)]
pub enum VfsError {
    NotFound,
    NotDir,
    AlreadyExists,
    Io,
    Invalid,
    ReadOnly,
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub ino: u32,
    pub uid: u32,
    pub gid: u32,
    pub name: String,
    pub file_type: FileType,
    pub mode: u16,
    pub size: usize,
    pub created_time: u32,
    pub access_time: u32,
    pub modified_time: u32,
}

impl Metadata {
    pub(crate) fn dir(ino: u32, name: &str) -> Self {
        Metadata {
            ino,
            uid: 0,
            gid: 0,
            name: name.into(),
            file_type: FileType::Dir,
            mode: 0o040755,
            size: 0,
            access_time: 0,
            created_time: 0,
            modified_time: 0,
        }
    }
    pub(crate) fn chr(ino: u32, name: &str) -> Self {
        Metadata {
            ino,
            name: name.into(),
            gid: 0,
            uid: 0,
            file_type: FileType::CharDevice,
            mode: 0o020666,
            size: 0,
            access_time: 0,
            created_time: 0,
            modified_time: 0,
        }
    }
    pub(crate) fn blk(ino: u32, name: &str, size: usize) -> Self {
        Metadata {
            ino,
            name: name.into(),
            file_type: FileType::BlockDevice,
            mode: 0o060660,
            size,
            access_time: 0,
            uid: 0,
            gid: 0,
            created_time: 0,
            modified_time: 0,
        }
    }
}
pub type BlockDev = Arc<Mutex<Box<dyn BlockDeviceIO + Send>>>;

#[derive(Debug, Clone, Copy, Default)]
pub struct FsStats {
    pub fs_type: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub blocks_free: u64,
    pub blocks_available: u64,
    pub files: u64,
    pub files_free: u64,
    pub name_length: u64,
    pub fragment_size: u64,
    pub flags: u64,
}

pub trait FsCtx {
    fn block_size(&self) -> usize;

    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()>;
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()>;

    fn alloc_zone(&mut self) -> Result<u32, TfsError>;
    fn free_zone(&mut self, zone: u32) -> Result<(), TfsError>;
    fn write_inode_twilight(&mut self, ino: u32, inode: Inode) -> Result<(), &'static str>;

    fn remove_file(&mut self, path: &str) -> Result<(), ()>;

    fn alloc_zones(&mut self, count: usize) -> Result<Vec<u32>, TfsError> {
        let mut zones = Vec::with_capacity(count);
        for _ in 0..count {
            match self.alloc_zone() {
                Ok(z) => zones.push(z),
                Err(e) => {
                    // TODO: Rollback allocated zones?
                    // For now, return error, but this might leak.
                    // ideally we should free them.
                    for z in zones {
                        let _ = self.free_zone(z);
                    }
                    return Err(e);
                }
            }
        }
        Ok(zones)
    }

    fn write_blocks(&mut self, start_lba: u32, buf: &[u8]) -> Result<(), ()> {
        let block_size = self.block_size();
        if buf.len() % block_size != 0 {
            return Err(());
        }
        let blocks = buf.len() / block_size;
        for i in 0..blocks {
            let offset = i * block_size;
            self.write_block(start_lba + i as u32, &buf[offset..offset + block_size])?;
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub struct VfsNode {
    pub device: BlockDev,
    pub metadata: Metadata,
    pub node: Arc<RwLock<dyn VfsNodeOps>>,
}

impl VfsNode {
    pub fn new(device: BlockDev, metadata: Metadata, node: Arc<RwLock<dyn VfsNodeOps>>) -> Self {
        Self {
            device,
            metadata,
            node,
        }
    }

    pub fn read(&mut self, lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        self.node.read().read(&mut self.device, lba, buf)
    }

    pub fn read_exact(&mut self, mut offset: usize, mut buf: &mut [u8]) -> Result<(), ()> {
        while !buf.is_empty() {
            let read = self.read(offset, buf)?;
            if read == 0 {
                return Err(());
            }
            offset = offset.checked_add(read).ok_or(())?;
            buf = &mut buf[read..];
        }
        Ok(())
    }

    pub fn write(&mut self, lba: usize, data: &[u8]) -> Result<(), ()> {
        self.node.write().write(&mut self.device, lba, data)
    }

    pub fn poll(&mut self) -> Result<bool, ()> {
        self.node.read().poll(&mut self.device)
    }

    pub fn unlink(&mut self) -> Result<i32, ()> {
        self.node.write().unlink(&mut self.device)
    }

    pub fn ioctl(&mut self, cmd: u64, arg: usize) -> Result<i64, ()> {
        self.node.write().ioctl(&mut self.device, cmd, arg)
    }

    pub fn mmap(
        &mut self,
        process: &mut Process,
        addr: usize,
        len: usize,
        prot: usize,
        flags: usize,
        offset: usize,
    ) -> Result<usize, i32> {
        self.node
            .write()
            .mmap(&mut self.device, process, addr, len, prot, flags, offset)
    }

    pub fn truncate(&mut self, len: usize) -> Result<(), i32> {
        self.node.write().truncate(&mut self.device, len)?;
        self.metadata.size = len;
        Ok(())
    }
}

impl Clone for VfsNode {
    fn clone(&self) -> Self {
        Self {
            device: self.device.clone(),
            metadata: self.metadata.clone(),
            node: self.node.clone(),
        }
    }
}

pub trait VfsNodeOps: Send + Sync + 'static {
    fn read(&self, device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<usize, ()>;
    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()>;
    fn poll(&self, device: &mut BlockDev) -> Result<bool, ()>;
    fn ioctl(&mut self, device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()>;
    fn unlink(&mut self, device: &mut BlockDev) -> Result<i32, ()>;
    fn truncate(&mut self, _device: &mut BlockDev, _len: usize) -> Result<(), i32> {
        Err(-38)
    }
    fn mmap(
        &mut self,
        _device: &mut BlockDev,
        _process: &mut Process,
        _addr: usize,
        _len: usize,
        _prot: usize,
        _flags: usize,
        _offset: usize,
    ) -> Result<usize, i32> {
        Err(-38)
    }
}

pub trait FileSystem: Send + Sync + 'static {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()>;
    fn mkdir(&mut self, parent_dir: &str, path: &str, mode: u16) -> Result<(), ()>;
    fn rmdir(&mut self, path: &str) -> Result<(), ()>;
    fn rename(&mut self, _old_path: &str, _new_path: &str) -> Result<(), ()> {
        Err(())
    }
    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()>;
    fn rm(&mut self, path: &str) -> Result<(), ()>;
    fn touch(&mut self, parent_path: &str, filename: &str, mode: u16) -> Result<(), ()>;
    fn metadata(&mut self, path: &str) -> Result<Metadata, ()>;
    fn chmod(&mut self, _path: &str, _mode: u16) -> Result<(), VfsError> {
        Err(VfsError::ReadOnly)
    }
    fn set_attr(&mut self, _path: &str, _attr: u32, _value: u32) -> Result<(), ()> {
        Err(())
    }
    fn get_attr(&mut self, _path: &str, _attr: u32) -> Result<u32, ()> {
        Err(())
    }
    fn fs_type_name(&self) -> &'static str {
        "unknown"
    }
    fn source_name(&self) -> &'static str {
        "none"
    }
    fn stats(&mut self) -> Result<FsStats, ()> {
        Ok(FsStats {
            block_size: 4096,
            name_length: 255,
            fragment_size: 4096,
            ..FsStats::default()
        })
    }
}

pub struct MountPoint {
    pub prefix: &'static str,
    pub fs: Arc<Mutex<dyn FileSystem>>,
    pub fs_type: &'static str,
    pub source: &'static str,
}

pub struct Vfs {
    pub mount_points: Vec<MountPoint>,
}

unsafe impl Send for Vfs {}
unsafe impl Sync for Vfs {}

#[allow(dead_code)]
impl Vfs {
    pub const fn new() -> Self {
        Self {
            mount_points: Vec::new(),
        }
    }

    pub fn mount(&mut self, prefix: &'static str, fs: Arc<Mutex<dyn FileSystem>>) {
        let (fs_type, source) = {
            let guard = fs.lock();
            (guard.fs_type_name(), guard.source_name())
        };
        self.mount_points.push(MountPoint {
            prefix,
            fs,
            fs_type,
            source,
        });
        self.mount_points
            .sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));
    }

    pub fn unmount(&mut self, prefix: &str) -> bool {
        if let Some(i) = self
            .mount_points
            .iter()
            .position(|mount| mount.prefix == prefix)
        {
            self.mount_points.remove(i);
            true
        } else {
            false
        }
    }

    #[inline]
    fn route<'a>(&self, path: &'a str) -> Option<(&'a str, &Arc<Mutex<dyn FileSystem>>)> {
        self.mount_points
            .iter()
            .find(|mount| {
                path.starts_with(mount.prefix) || ((path == ".") && (mount.prefix == "/"))
            })
            .map(|mount| {
                let rel = &path[mount.prefix.len()..];
                (if rel.is_empty() { "/" } else { rel }, &mount.fs)
            })
    }

    pub fn open(&self, path: &str) -> Result<VfsNode, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.open(rel)
    }

    pub fn mkdir(&self, parent_path: &str, path: &str, mode: u16) -> Result<(), ()> {
        let (rel, fs) = self.route(parent_path).ok_or(())?;
        let mut guard = fs.lock();
        guard.mkdir(rel, path, mode)
    }

    pub fn rmdir(&self, path: &str) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.rmdir(rel)
    }

    pub fn rename(&self, old_path: &str, new_path: &str) -> Result<(), ()> {
        let (old_rel, old_fs) = self.route(old_path).ok_or(())?;
        let (new_rel, new_fs) = self.route(new_path).ok_or(())?;
        if !Arc::ptr_eq(old_fs, new_fs) {
            return Err(());
        }
        let mut guard = old_fs.lock();
        guard.rename(old_rel, new_rel)
    }

    pub fn ls(&self, path: &str) -> Result<Vec<Metadata>, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.ls(rel)
    }

    pub fn rm(&self, path: &str) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.rm(rel)
    }

    pub fn touch(&self, parent_path: &str, filename: &str, mode: u16) -> Result<(), ()> {
        let (rel, fs) = self.route(parent_path).ok_or(())?;
        let mut guard = fs.lock();
        guard.touch(rel, filename, mode)
    }

    pub fn metadata(&self, path: &str) -> Result<Metadata, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.metadata(rel)
    }

    pub fn chmod(&self, path: &str, mode: u16) -> Result<(), VfsError> {
        let (rel, fs) = self.route(path).ok_or(VfsError::NotFound)?;
        fs.lock().chmod(rel, mode)
    }

    pub fn stats(&self, path: &str) -> Result<FsStats, ()> {
        let (_, fs) = self.route(path).ok_or(())?;
        fs.lock().stats()
    }

    pub fn set_attr(&self, path: &str, attr: u32, value: u32) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.set_attr(rel, attr, value)
    }

    pub fn get_attr(&self, path: &str, attr: u32) -> Result<u32, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.get_attr(rel, attr)
    }
}

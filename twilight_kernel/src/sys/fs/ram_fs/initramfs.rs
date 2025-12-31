pub struct CpioNewCHeader {
    pub c_magic: [u8; 6],
    pub c_ino: [u8; 8],
    pub c_mode: [u8; 8],
    pub c_uid: [u8; 8],
    pub c_gid: [u8; 8],
    pub c_nlink: [u8; 8],
    pub c_mtime: [u8; 8],
    pub c_filesize: [u8; 8],
    pub c_devmajor: [u8; 8],
    pub c_devminor: [u8; 8],
    pub c_rdevmajor: [u8; 8],
    pub c_rdevminor: [u8; 8],
    pub c_namesize: [u8; 8],
    pub c_check: [u8; 8],
}

impl CpioNewCHeader {
    pub const SIZE: usize = 110;

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < Self::SIZE {
            return None;
        }
        // Header layout is fixed: 6 + 13*8
        let mut off = 0usize;

        let mut c_magic = [0u8; 6];
        c_magic.copy_from_slice(&buf[off..off + 6]);
        off += 6;

        let mut read8 = |arr: &mut [u8; 8]| {
            arr.copy_from_slice(&buf[off..off + 8]);
            off += 8;
        };

        let mut c_ino = [0u8; 8];
        read8(&mut c_ino);
        let mut c_mode = [0u8; 8];
        read8(&mut c_mode);
        let mut c_uid = [0u8; 8];
        read8(&mut c_uid);
        let mut c_gid = [0u8; 8];
        read8(&mut c_gid);
        let mut c_nlink = [0u8; 8];
        read8(&mut c_nlink);
        let mut c_mtime = [0u8; 8];
        read8(&mut c_mtime);
        let mut c_filesize = [0u8; 8];
        read8(&mut c_filesize);
        let mut c_devmajor = [0u8; 8];
        read8(&mut c_devmajor);
        let mut c_devminor = [0u8; 8];
        read8(&mut c_devminor);
        let mut c_rdevmajor = [0u8; 8];
        read8(&mut c_rdevmajor);
        let mut c_rdevminor = [0u8; 8];
        read8(&mut c_rdevminor);
        let mut c_namesize = [0u8; 8];
        read8(&mut c_namesize);
        let mut c_check = [0u8; 8];
        read8(&mut c_check);

        Some(Self {
            c_magic,
            c_ino,
            c_mode,
            c_uid,
            c_gid,
            c_nlink,
            c_mtime,
            c_filesize,
            c_devmajor,
            c_devminor,
            c_rdevmajor,
            c_rdevminor,
            c_namesize,
            c_check,
        })
    }

    pub fn namesize(&self) -> Option<u64> {
        hex_to_u64(&self.c_namesize)
    }
    pub fn filesize(&self) -> Option<u64> {
        hex_to_u64(&self.c_filesize)
    }
    pub fn mode(&self) -> Option<u64> {
        hex_to_u64(&self.c_mode)
    }

    pub fn is_regular_file(&self) -> bool {
        self.mode().map_or(false, |m| (m & 0o170000) == 0o100000)
    }

    pub fn is_directory(&self) -> bool {
        self.mode().map_or(false, |m| (m & 0o170000) == 0o040000)
    }

    pub fn is_symlink(&self) -> bool {
        self.mode().map_or(false, |m| (m & 0o170000) == 0o120000)
    }
}

fn hex_to_u64(b: &[u8; 8]) -> Option<u64> {
    let mut acc: u64 = 0;
    for &c in b.iter() {
        acc = acc.checked_shl(4)?; // multiply by 16, avoid overflow
        let v = match c {
            b'0'..=b'9' => (c - b'0') as u64,
            b'a'..=b'f' => (c - b'a' + 10) as u64,
            b'A'..=b'F' => (c - b'A' + 10) as u64,
            _ => return None,
        };
        acc |= v;
    }
    Some(acc)
}

pub struct CpioEntry<'a> {
    pub header: CpioNewCHeader,
    /// Filename including trailing NUL (namesize bytes). Use `filename()` to get &str without trailing NUL.
    raw_name: &'a [u8],
    /// File data bytes (length = header.filesize())
    pub data: &'a [u8],
}

impl<'a> CpioEntry<'a> {
    /// Returns filename as &str (without the trailing NUL). If not valid UTF-8, returns `None`.
    pub fn filename(&self) -> Option<&'a str> {
        // namesize includes trailing NUL; strip last byte if it is NUL.
        let mut name = self.raw_name;
        if !name.is_empty() && name[name.len() - 1] == 0 {
            name = &name[..name.len() - 1];
        }
        str::from_utf8(name).ok()
    }
}

#[derive(Clone)]
pub struct CpioIterator {
    buf: &'static [u8],
    pos: usize,
}

#[derive(Debug)]
pub enum CpioError {
    Truncated,
    BadMagic,
    HexParseFailed,
    Trailer,
}

impl CpioIterator {
    pub fn new(buf: &'static [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn default() -> Self {
        Self { buf: &[], pos: 0 }
    }

    /// Helper to align an offset up to 4 bytes
    fn align4(x: usize) -> usize {
        (4 - (x & 3)) & 3
    }
}

impl Iterator for CpioIterator {
    type Item = Result<CpioEntry<'static>, CpioError>;

    fn next(&mut self) -> Option<Self::Item> {
        // If we've reached or exceeded buffer length, end iteration.
        if self.pos >= self.buf.len() {
            return None;
        }

        // Ensure header available
        if self.buf.len() - self.pos < CpioNewCHeader::SIZE {
            return Some(Err(CpioError::Truncated));
        }

        let hdr = match CpioNewCHeader::from_bytes(&self.buf[self.pos..]) {
            Some(h) => h,
            None => return Some(Err(CpioError::Truncated)),
        };

        // Validate magic
        if &hdr.c_magic != b"070701" {
            return Some(Err(CpioError::BadMagic));
        }

        // Parse numeric fields
        let namesize = match hdr.namesize() {
            Some(n) => n as usize,
            None => return Some(Err(CpioError::HexParseFailed)),
        };
        let filesize = match hdr.filesize() {
            Some(n) => n as usize,
            None => return Some(Err(CpioError::HexParseFailed)),
        };

        // name starts immediately after header
        let name_start = self.pos + CpioNewCHeader::SIZE;
        let name_end = name_start.checked_add(namesize);
        if name_end.is_none() || name_end.unwrap() > self.buf.len() {
            return Some(Err(CpioError::Truncated));
        }
        let name_end = name_end.unwrap();
        let raw_name = &self.buf[name_start..name_end];

        // Name + header padded to 4 bytes
        let name_padding = CpioIterator::align4(CpioNewCHeader::SIZE + namesize);
        let file_data_start = name_end + name_padding;
        if file_data_start > self.buf.len() {
            return Some(Err(CpioError::Truncated));
        }

        // File data bytes
        let file_data_end = file_data_start.checked_add(filesize);
        if file_data_end.is_none() || file_data_end.unwrap() > self.buf.len() {
            return Some(Err(CpioError::Truncated));
        }
        let file_data_end = file_data_end.unwrap();
        let data = &self.buf[file_data_start..file_data_end];

        // Check for trailer
        if raw_name == b"TRAILER!!!\0" || raw_name == b"TRAILER!!!" {
            // Move pos to end and return Trailer error (so caller can stop)
            self.pos = self.buf.len();
            return Some(Err(CpioError::Trailer));
        }

        // Compute padding after file data to align next header
        let file_padding = CpioIterator::align4(filesize);
        let next_pos = file_data_end + file_padding;

        // Prepare return
        let entry = CpioEntry {
            header: hdr,
            raw_name,
            data,
        };

        // Advance iterator state
        self.pos = next_pos;

        Some(Ok(entry))
    }
}

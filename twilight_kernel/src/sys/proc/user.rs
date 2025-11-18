use alloc::{format, vec};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;
use crate::serial_println;
use crate::sys::fs::vfs::VFS;

lazy_static! {
    pub static ref USER_ENV: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
}
pub static USER_ID: Mutex<usize> = Mutex::new(0);

pub fn set_uid(uid: usize) {
    *USER_ID.lock() = uid;
}

pub fn get_uid() -> usize {
    *USER_ID.lock()
}

pub fn set_user_env() {
    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    let Ok(mut node) = fs.open("/etc/passwd") else {
        return;
    };

    serial_println!("{}", get_uid());

    let mut buf = vec![0u8; node.metadata.size];
    node.read(0, &mut buf).unwrap();

    let content = String::from_utf8(buf).unwrap();

    for entry in content.lines() {
        if let Some(entry) = parse_passwd_line(entry) {
            if entry.uid == get_uid() {
                USER_ENV.lock().push(format!("USER={}", entry.name.as_str()));
            }
        }
    }
    
}


#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PasswdEntry {
    name: String,
    _passwd: String,
    uid: usize,
    gid: usize,
    gecos: String,
    home: String,
    shell: String,
}

fn parse_passwd_line(line: &str) -> Option<PasswdEntry> {
    // Format: name:passwd:uid:gid:gecos:home:shell
    let mut parts = line.splitn(7, ':');
    let name   = parts.next()?.to_string();
    let pw     = parts.next()?.to_string();
    let uid_s  = parts.next()?;
    let gid_s  = parts.next()?;
    let gecos  = parts.next().unwrap_or_default().to_string();
    let home   = parts.next().unwrap_or("/").to_string();
    let shell  = parts.next().unwrap_or("/bin/tsh").trim_end().to_string(); // trim '\n'

    let uid = uid_s.parse().ok()?;
    let gid = gid_s.parse().ok()?;

    Some(PasswdEntry { name, _passwd: pw, uid, gid, gecos, home, shell })
}

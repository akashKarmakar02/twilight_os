use crate::sys::fs::vfs::VFS;
use crate::sys::proc::PROCESS_TABLE;
use crate::sys::syscall::utils::{UserPtr, copy_cstr_from_user};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use twilight_common::syscall::types::{EACCES, EFAULT, EINVAL, EIO, ENOENT};

pub const IFLAG_ENCRYPTED: u32 = 1 << 2;

fn join_paths(base: &str, rel: &str) -> String {
    if rel.is_empty() || rel == "." {
        return base.to_string();
    }
    if rel.starts_with('/') {
        return rel.to_string();
    }
    if base == "/" {
        format!("/{}", rel.trim_start_matches('/'))
    } else {
        format!("{}/{}", base.trim_end_matches('/'), rel)
    }
}

fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            out.pop();
        } else {
            out.push(seg);
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

fn resolve_full_path(path: &str) -> Result<String, isize> {
    if path.starts_with('/') {
        return Ok(normalize_path(path));
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return Err(-(EIO as isize));
    };

    Ok(normalize_path(&join_paths(&process.pwd, path)))
}

pub fn sys_set_file_attr(path_ptr: *const u8, attr: u32, value: u32) -> isize {
    if attr != IFLAG_ENCRYPTED {
        return -(EINVAL as isize);
    }
    if value == 0 {
        return -(EINVAL as isize);
    }

    let upath = UserPtr(path_ptr);
    let path = match copy_cstr_from_user(upath, 4096) {
        Ok(s) => s,
        _ => return -(EFAULT as isize),
    };
    let full_path = match resolve_full_path(path.as_str()) {
        Ok(p) => p,
        Err(e) => return e,
    };

    #[allow(static_mut_refs)]
    let meta = match unsafe { VFS.get_mut().metadata(&full_path) } {
        Ok(m) => m,
        Err(_) => return -(ENOENT as isize),
    };
    let current_uid = crate::sys::proc::user::get_uid() as u32;
    if current_uid != 0 && current_uid != meta.uid {
        return -(EACCES as isize);
    }

    #[allow(static_mut_refs)]
    match unsafe { VFS.get_mut().set_attr(&full_path, attr, value) } {
        Ok(_) => 0,
        Err(_) => -(EIO as isize),
    }
}

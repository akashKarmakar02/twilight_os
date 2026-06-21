use alloc::collections::BTreeMap;
use crate::utils::sync::RwLock;

static USER_KEYS: RwLock<BTreeMap<u32, [u8; 32]>> = RwLock::new(BTreeMap::new());

pub fn sys_add_user_key(uid: u32, key_ptr: *const u8, key_len: usize) -> isize {
    // Basic validation
    if key_len != 32 || key_ptr.is_null() {
        return -1; // EINVAL
    }

    // Copy key from userspace
    let mut key = [0u8; 32];
    // SAFETY: We should validate user pointer properly (e.g., using `UserSlice` abstraction if available).
    // For this step, assuming flat flat addressing or simple validation.
    // In a real OS, use copy_from_user.
    unsafe {
        core::ptr::copy_nonoverlapping(key_ptr, key.as_mut_ptr(), 32);
    }

    USER_KEYS.write().insert(uid, key);
    0
}

pub fn get_user_key(uid: u32) -> Option<[u8; 32]> {
    USER_KEYS.read().get(&uid).copied()
}

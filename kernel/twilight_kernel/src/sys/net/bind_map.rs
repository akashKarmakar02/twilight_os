use alloc::collections::BTreeMap;
use crate::utils::sync::Mutex;

pub static mut GLOBAL_PORT_MAP: Mutex<BTreeMap<u16, u16>> = Mutex::new(BTreeMap::new());

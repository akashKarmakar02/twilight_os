use lazy_static::lazy_static;
use spin::Mutex;

lazy_static! {
    pub static ref TICKS: Mutex<usize> = Mutex::new(0);
}


const PIT_FREQUENCY: f64 = 1_193_182.0;
const PIT_DIVISOR: f64 = 65_536.0;
const TICK_DURATION: f64 = 1.0 / (PIT_FREQUENCY / PIT_DIVISOR); // ≈ 0.0549 sec per tick


pub fn tick() {
    *TICKS.lock() += 1;
}

pub fn uptime() -> f64 {
    let ticks = *TICKS.lock();
    ticks as f64 * TICK_DURATION
}

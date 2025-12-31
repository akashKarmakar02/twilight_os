use crate::driver::timer::cmos::CMOS;

pub fn epoch_time() -> f64 {
    let mut cmos = CMOS::new();

    cmos.unix_time() as f64
}

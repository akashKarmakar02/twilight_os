use crate::{println, print};

pub fn main() {
    println!("Twilight OS will shutdown now...");
    crate::executor::sleep(2f64);
    crate::arch::x86_64::power::poweroff();
}
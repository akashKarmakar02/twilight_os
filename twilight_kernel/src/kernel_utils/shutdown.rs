use crate::println;

pub fn main() {
    println!("Twilight OS will shutdown now...");
    crate::executor::sleep(2f64);
    crate::sys::acpi::shutdown();
}
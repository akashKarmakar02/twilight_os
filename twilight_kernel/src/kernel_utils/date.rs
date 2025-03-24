use crate::{driver::timer::cmos::CMOS, print, println};

pub fn main() {
    let mut rtc = CMOS::new();

    let date = rtc.read();
    println!(
        "{} {} {}:{}:{} {} GMT",
        get_month_name(date.month),
        date.day,
        date.hour,
        date.minute,
        date.second,
        2000 + date.year as usize
    );
}

fn get_month_name(month: u8) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Invalid Month",
    }
}

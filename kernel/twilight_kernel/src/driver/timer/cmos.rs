use crate::print;
use x86_64::instructions::port::Port;

#[repr(u8)]
pub enum Register {
    Second = 0x00,
    Minute = 0x02,
    Hour = 0x04,
    Day = 0x07,
    Month = 0x08,
    Year = 0x09,
    B = 0x0B,
}

#[derive(Debug)]
pub struct RTC {
    pub second: u8,
    pub minute: u8,
    pub hour: u8,
    pub day: u8,
    pub month: u8,
    pub year: u8,
}

pub struct CMOS {
    addr: Port<u8>,
    data: Port<u8>,
}

impl Default for CMOS {
    fn default() -> Self {
        Self::new()
    }
}

impl CMOS {
    pub const fn new() -> Self {
        Self {
            addr: Port::new(0x70),
            data: Port::new(0x71),
        }
    }

    pub fn unix_time(&mut self) -> u64 {
        let rtc = self.read();

        // Convert the year to full 4-digit form (assumes 20xx)
        let year = 2000 + rtc.year as u64;
        let month = rtc.month as u64;
        let day = rtc.day as u64;
        let hour = rtc.hour as u64;
        let minute = rtc.minute as u64;
        let second = rtc.second as u64;

        // Days in months, not accounting for leap years yet
        let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

        // Calculate number of days since Unix epoch
        let mut days = 0;

        // Add days for all previous years
        for y in 1970..year {
            days += if is_leap_year(y) { 366 } else { 365 };
        }

        // Add days for all previous months in the current year
        for m in 0..(month - 1) {
            days += days_in_month[m as usize];
            if m == 1 && is_leap_year(year) {
                days += 1; // February in a leap year
            }
        }

        // Add days in current month
        days += day - 1;

        // Convert everything to seconds
        let total_seconds = days * 86400 + hour * 3600 + minute * 60 + second;

        total_seconds
    }

    pub fn read(&mut self) -> RTC {
        while self.is_updating() {
            print!("");
        }

        let mut second = self.read_register(Register::Second);
        let mut minute = self.read_register(Register::Minute);
        let mut hour = self.read_register(Register::Hour);
        let mut day = self.read_register(Register::Day);
        let mut month = self.read_register(Register::Month);
        let mut year = self.read_register(Register::Year);

        let b = self.read_register(Register::B);

        if b & 0x04 == 0 {
            second = (second & 0x0F) + ((second / 16) * 10);
            minute = (minute & 0x0F) + ((minute / 16) * 10);
            hour = ((hour & 0x0F) + (((hour & 0x70) / 16) * 10)) | (hour & 0x80);
            day = (day & 0x0F) + ((day / 16) * 10);
            month = (month & 0x0F) + ((month / 16) * 10);
            year = (year & 0x0F) + ((year / 16) * 10);
        }

        RTC {
            second,
            minute,
            hour,
            day,
            month,
            year,
        }
    }

    fn is_updating(&mut self) -> bool {
        unsafe {
            self.addr.write(0x0A_u8);
            self.data.read() & 0x80 == 1
        }
    }

    pub fn read_register(&mut self, register: Register) -> u8 {
        unsafe {
            self.addr.write(register as u8);
            self.data.read()
        }
    }
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

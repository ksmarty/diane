#![no_std]

use embassy_time::Instant;
use embedded_sdmmc::{TimeSource, Timestamp};

pub struct ESPTime {
    start: Instant,
}

impl ESPTime {
    pub fn new() -> ESPTime {
        Self {
            start: Instant::now(),
        }
    }
}

impl TimeSource for ESPTime {
    fn get_timestamp(&self) -> embedded_sdmmc::Timestamp {
        let now = &self.start.as_secs();
        Timestamp {
            year_since_1970: (now / 31536000) as u8,
            zero_indexed_month: ((now / 2628000) % 12) as u8,
            zero_indexed_day: ((now / 86400) % 30) as u8,
            hours: ((now / 3600) % 24) as u8,
            minutes: ((now / 60) % 60) as u8,
            seconds: (now % 60) as u8,
        }
    }
}

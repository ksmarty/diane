use embedded_sdmmc::Timestamp;
use utc_dt::{
    date::UTCDate,
    time::{UTCTimeOfDay, UTCTimestamp},
};

// Month name to number mapping
const MONTHS: [&[u8]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

pub struct MiniDateTime {
    http_date: [u8; 29],
    pub sd_timestamp: Timestamp,
    pub unix_timestamp: u64,
}

impl MiniDateTime {
    pub fn new(input: &str) -> Self {
        let mut http_date = [0u8; 29];
        http_date.copy_from_slice(input.as_bytes());

        let unix_timestamp = parse_http_date(&http_date).unwrap();
        let sd_timestamp = timestamp_from_unix(unix_timestamp);

        MiniDateTime {
            http_date,
            sd_timestamp,
            unix_timestamp,
        }
    }
}

fn parse_http_date(input: &[u8]) -> Option<u64> {
    // Example format: "Sun, 06 Nov 1994 08:49:37 GMT"
    if input.len() < 29 {
        return None;
    }

    // Skip weekday and comma (first 5 bytes)
    let input = &input[5..];

    // Parse components first
    let day = parse_number(&input[0..2])? as u8;
    let month = parse_month(&input[3..6])? as u8;
    let year = parse_number(&input[7..11])? as u64;
    let hour = parse_number(&input[12..14])? as u8;
    let minute = parse_number(&input[15..17])? as u8;
    let second = parse_number(&input[18..20])? as u8;

    // Validate ranges
    if day == 0 || day > 31 || hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }

    let today = unsafe { UTCDate::from_components_unchecked(year, month, day) }.as_day();
    let time = unsafe { UTCTimeOfDay::from_hhmmss_unchecked(hour, minute, second, 0) };
    let timestamp = UTCTimestamp::from_day_and_tod(today, time);

    Some(timestamp.as_secs())
}

fn timestamp_from_unix(unix_timestamp: u64) -> Timestamp {
    let timestamp = UTCTimestamp::from_secs(unix_timestamp);
    let (year, month, day) = UTCDate::from(timestamp).as_components();
    let (hours, minutes, seconds) = UTCTimeOfDay::from_timestamp(timestamp).as_hhmmss();

    Timestamp {
        year_since_1970: (year - 1970) as u8,
        zero_indexed_month: (month - 1),
        zero_indexed_day: (day - 1),
        hours,
        minutes,
        seconds,
    }
}

fn parse_number(input: &[u8]) -> Option<u16> {
    let mut result = 0u16;
    for &digit in input {
        if !digit.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?;
        result = result.checked_add((digit - b'0') as u16)?;
    }
    Some(result)
}

fn parse_month(input: &[u8]) -> Option<u8> {
    for (i, &month) in MONTHS.iter().enumerate() {
        if input == month {
            return Some((i + 1) as u8);
        }
    }
    None
}

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use embassy_time::Instant;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{Directory, SdCard, TimeSource, Timestamp};
use esp_hal::{
    delay::Delay,
    gpio::{self, AnyPin, Output},
    rtc_cntl::{
        sleep::{RtcioWakeupSource, WakeupLevel},
        Rtc,
    },
    spi::master::Spi,
    Blocking,
};

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

pub struct SDUtils {}

impl SDUtils {
    pub fn get_next_file_name(
        root_dir: &mut Directory<
            '_,
            SdCard<ExclusiveDevice<Spi<'_, Blocking>, Output<'_>, Delay>, Delay>,
            ESPTime,
            4,
            4,
            1,
        >,
    ) -> String {
        let mut highest_number = 0;
        root_dir
            .iterate_dir(|e| {
                let name = String::from_utf8(e.name.base_name().to_vec()).unwrap();
                let number = name.parse::<u32>().ok().unwrap_or(0);
                highest_number = highest_number.max(number);
            })
            .unwrap();

        (highest_number + 1).to_string() + &".wav"
    }
}

pub fn enter_deep_sleep(mut gpio3: AnyPin, lpwr: esp_hal::peripherals::LPWR) {
    let wakeup_pins: &mut [(&mut dyn gpio::RtcPinWithResistors, WakeupLevel)] =
        &mut [(&mut gpio3, WakeupLevel::Low)];

    let wake_source = RtcioWakeupSource::new(wakeup_pins);

    let mut rtc = Rtc::new(lpwr);
    rtc.sleep_deep(&[&wake_source])
}

pub const HEADER_SIZE: usize = 44;
pub const HEADER: [u8; HEADER_SIZE] = [
    b'R', b'I', b'F', b'F', // ChunkID
    0xff, 0xff, 0xff, 0xff, // ChunkSize (to be filled)
    b'W', b'A', b'V', b'E', // Format
    b'f', b'm', b't', b' ', // Subchunk1ID
    16, 0, 0, 0, // Subchunk1Size (16 for PCM)
    1, 0, // AudioFormat (1 = PCM)
    2, 0, // NumChannels (1 = Mono)
    0x80, 0x3E, 0x00, 0x00, // SampleRate (16 kHz)
    0x00, 0xFA, 0x00, 0x00, // ByteRate
    4, 0, // BlockAlign
    16, 0, // BitsPerSample
    b'd', b'a', b't', b'a', // Subchunk2ID
    0xff, 0xff, 0xff, 0xff, // Subchunk2Size (to be filled)
];

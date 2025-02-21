#![no_std]

pub mod http_date_time;
pub mod wifi_helper;

extern crate alloc;

use core::u8;

use alloc::string::{String, ToString};
use embassy_time::Instant;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::filesystem::ToShortFileName;
use embedded_sdmmc::{BlockDevice, Directory, File, SdCard, TimeSource, Timestamp, VolumeManager};
use esp_hal::{
    delay::Delay,
    dma_buffers,
    gpio::{self, AnyPin, Level, Output},
    i2s::master::{DataFormat, I2s, I2sRx, Standard},
    rtc_cntl::{
        sleep::{RtcioWakeupSource, WakeupLevel},
        Rtc,
    },
    spi::master::{Config, Spi},
    time::RateExtU32,
    Blocking,
};
use esp_println::println;

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
    pub fn setup_sd_card<'a, 'b>(
        sclk: AnyPin,
        miso: AnyPin,
        mosi: AnyPin,
        cs_pin: AnyPin,
        spi2: esp_hal::peripherals::SPI2,
    ) -> VolumeManager<
        SdCard<ExclusiveDevice<Spi<'a, Blocking>, Output<'b>, Delay>, Delay>,
        ESPTime,
        4,
        4,
        1,
    > {
        let cs = Output::new(cs_pin, Level::Low);

        let spi = Spi::new(
            spi2,
            Config::default()
                .with_frequency(25.MHz())
                .with_mode(esp_hal::spi::Mode::_0),
        )
        .unwrap()
        .with_sck(sclk)
        .with_mosi(mosi)
        .with_miso(miso);

        let delay = Delay::new();

        let spi_dev = ExclusiveDevice::new(spi, cs, delay).unwrap();

        let sd_card = SdCard::new(spi_dev, delay);

        println!("Size of the sd card: {:#?}", sd_card.num_bytes().unwrap());

        let volume_manager = VolumeManager::new(sd_card, ESPTime::new());

        volume_manager
    }

    pub fn get_next_file_name(
        root_dir: &mut Directory<
            '_,
            SdCard<ExclusiveDevice<Spi<'_, Blocking>, Output<'_>, Delay>, Delay>,
            ESPTime,
            4,
            4,
            1,
        >,
    ) -> embedded_sdmmc::ShortFileName {
        let mut highest_number = 0;
        root_dir
            .iterate_dir(|e| {
                let name = String::from_utf8(e.name.base_name().to_vec()).unwrap();
                let number = name.parse::<u32>().ok().unwrap_or(0);
                highest_number = highest_number.max(number);
            })
            .unwrap();

        let name = (highest_number + 1).to_string() + &".wav";

        name.to_short_filename().unwrap()
    }

    pub fn write_wav_header<'a, 'b, 'c, 'd, 'e>(
        file: &'a mut File<
            '_,
            SdCard<ExclusiveDevice<Spi<'_, Blocking>, Output<'_>, Delay>, Delay>,
            ESPTime,
            4,
            4,
            1,
        >
    ) -> Result<(), embedded_sdmmc::Error<<SdCard<ExclusiveDevice<Spi<'d, Blocking>, Output<'e>, Delay>, Delay> as BlockDevice>::Error>>{
        file.write(&HEADER)?;
        file.flush()?;

        Ok(())
    }

    pub fn update_wav_header<'a, 'b, 'c, 'd, 'e>(
        file: &'a mut File<
            '_,
            SdCard<ExclusiveDevice<Spi<'_, Blocking>, Output<'_>, Delay>, Delay>,
            ESPTime,
            4,
            4,
            1,
        >,
        file_size: u32,
    ) -> Result<(), embedded_sdmmc::Error<<SdCard<ExclusiveDevice<Spi<'d, Blocking>, Output<'e>, Delay>, Delay> as BlockDevice>::Error>>{
        file.seek_from_start(4)?;
        file.write(&(file_size - 8).to_ne_bytes())?;
        file.flush()?;

        file.seek_from_start(40)?;
        file.write(&(file_size - HEADER_SIZE as u32).to_ne_bytes())?;
        file.flush()?;

        Ok(())
    }
}

pub fn setup_i2s<'a>(
    peripherals_gpio0: AnyPin,
    peripherals_gpio2: AnyPin,
    peripherals_gpio1: AnyPin,
    peripherals_i2s0: esp_hal::peripherals::I2S0,
    peripherals_dma_ch1: esp_hal::dma::DmaChannel1,
) -> (I2sRx<'a, Blocking>, [u8; 4 * I2S_BYTES]) {
    let bclk = peripherals_gpio0; // sck - purple
    let din = peripherals_gpio2; //  sd  - black
    let ws = peripherals_gpio1; //   ws  - blue

    let dma_channel_i2s = peripherals_dma_ch1;
    let (rx_buffer_i2s, rx_descriptors_i2s, _, tx_descriptors_i2s) = dma_buffers!(4 * I2S_BYTES, 0);

    let i2s = I2s::new(
        peripherals_i2s0,
        Standard::Philips,
        DataFormat::Data16Channel16,
        16.kHz(),
        dma_channel_i2s,
        rx_descriptors_i2s,
        tx_descriptors_i2s,
    );

    let i2s_rx = i2s.i2s_rx.with_bclk(bclk).with_ws(ws).with_din(din).build();
    (i2s_rx, *rx_buffer_i2s)
}

pub fn enter_deep_sleep(mut gpio3: AnyPin, lpwr: esp_hal::peripherals::LPWR) {
    let wakeup_pins: &mut [(&mut dyn gpio::RtcPinWithResistors, WakeupLevel)] =
        &mut [(&mut gpio3, WakeupLevel::Low)];

    let wake_source = RtcioWakeupSource::new(wakeup_pins);

    let mut rtc = Rtc::new(lpwr);
    rtc.sleep_deep(&[&wake_source])
}

// DMA buffer size
pub const I2S_BYTES: usize = 4092;

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

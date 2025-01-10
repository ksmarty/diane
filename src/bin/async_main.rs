#![no_std]
#![no_main]

use diane::ESPTime;
use embassy_executor::Spawner;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::Mode;
use embedded_sdmmc::SdCard;
use embedded_sdmmc::VolumeManager;
use esp_backtrace as _;
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output},
    prelude::*,
    spi::{
        master::{Config, Spi},
        SpiMode,
    },
    timer::timg::TimerGroup,
};
use esp_println::println;

// Per DOS: file names are max: 8 name, 3 extension
const FILE_NAME: &str = "CONFIG.YML";

#[main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init({
        let mut config = esp_hal::Config::default();
        config.cpu_clock = CpuClock::max();
        config
    });

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    esp_println::logger::init_logger_from_env();

    let sclk = peripherals.GPIO18;
    let miso = peripherals.GPIO19;
    let mosi = peripherals.GPIO23;
    let cs = Output::new(peripherals.GPIO5, Level::Low);

    let spi = Spi::new_with_config(
        peripherals.SPI2,
        Config {
            frequency: 100.kHz(),
            mode: SpiMode::Mode0,
            ..Config::default()
        },
    )
    .with_sck(sclk)
    .with_mosi(mosi)
    .with_miso(miso);

    let delay = Delay::new();

    let spi_dev = ExclusiveDevice::new(spi, cs, delay).unwrap();
    let sd_card = SdCard::new(spi_dev, delay);

    println!("Size of the sd card: {:#?}", sd_card.num_bytes().unwrap());

    let mut volume_manager = VolumeManager::new(sd_card, ESPTime::new());

    let mut volume = volume_manager
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .unwrap();
    println!("Volume 0: {:?}", volume);

    let mut root_dir = volume.open_root_dir().unwrap();

    let mut file = root_dir
        .open_file_in_dir(FILE_NAME, Mode::ReadWriteCreate)
        .unwrap();
    let contents = b"
wifi:
    SSID: \"\"
    PASSWORD: \"\"
    ";
    file.write(contents).unwrap();

    file.close().unwrap();

    println!("File written!");
}

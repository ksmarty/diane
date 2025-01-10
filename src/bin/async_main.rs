#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::SdCard;
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
    let miso = peripherals.GPIO19; // This pin might be switched
    let mosi = peripherals.GPIO23; // with this pin, 🤷
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
}

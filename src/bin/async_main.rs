#![no_std]
#![no_main]

use core::u8;

use diane::{enter_deep_sleep, ESPTime, SDUtils, HEADER, HEADER_SIZE};
use embassy_executor::Spawner;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::filesystem::ToShortFileName;
use embedded_sdmmc::{Mode, SdCard, VolumeManager};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, Pin, Pull};
use esp_hal::i2s::master::{DataFormat, I2s, Standard};
use esp_hal::reset::wakeup_cause;
use esp_hal::rtc_cntl::{reset_reason, SocResetReason};
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output},
    spi::master::{Config, Spi},
    time::RateExtU32,
    timer::systimer::SystemTimer,
};
use esp_hal::{dma_buffers, Cpu};
use esp_hal_embassy::main;
use esp_println::println;
use log::info;

extern crate alloc;

// DMA buffer size
const I2S_BYTES: usize = 4092;

#[main]
async fn main(_spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(72 * 1024);

    esp_println::logger::init_logger_from_env();

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    info!("Embassy initialized!");

    let reason = reset_reason(Cpu::ProCpu).unwrap_or(SocResetReason::ChipPowerOn);

    let wake_reason = wakeup_cause();

    info!("{:?}", reason);
    info!("{:?}", wake_reason);

    let mut led = Output::new(peripherals.GPIO9, Level::Low);

    // let timer1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    // let _init = esp_wifi::init(
    //     timer1.timer0,
    //     esp_hal::rng::Rng::new(peripherals.RNG),
    //     peripherals.RADIO_CLK,
    // )
    // .unwrap();

    // -------------------
    // SD Card - Device Setup
    // -------------------

    let sclk = peripherals.GPIO4;
    let miso = peripherals.GPIO5;
    let mosi = peripherals.GPIO6;
    let cs = Output::new(peripherals.GPIO7, Level::Low);

    let spi = Spi::new(
        peripherals.SPI2,
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

    let mut volume_manager = VolumeManager::new(sd_card, ESPTime::new());

    let mut volume = volume_manager
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .unwrap();
    println!("Volume 0: {:?}", volume);

    // -------------------
    // i2s Setup
    // -------------------

    let bclk = peripherals.GPIO0; // sck - purple
    let din = peripherals.GPIO2; //  sd  - black
    let ws = peripherals.GPIO1; //   ws  - blue

    let dma_channel_i2s = peripherals.DMA_CH1;
    let (mut rx_buffer_i2s, rx_descriptors_i2s, _, tx_descriptors_i2s) =
        dma_buffers!(4 * I2S_BYTES, 0);

    let i2s = I2s::new(
        peripherals.I2S0,
        Standard::Philips,
        DataFormat::Data16Channel16,
        16.kHz(),
        dma_channel_i2s,
        rx_descriptors_i2s,
        tx_descriptors_i2s,
    );

    let mut i2s_rx = i2s.i2s_rx.with_bclk(bclk).with_ws(ws).with_din(din).build();

    let mut button = Input::new(peripherals.GPIO10, Pull::Up);

    let mut root_dir = volume.open_root_dir().unwrap();

    let file_name = SDUtils::get_next_file_name(&mut root_dir);

    let mut file = root_dir
        .open_file_in_dir(
            file_name.to_short_filename().unwrap(),
            Mode::ReadWriteCreate,
        )
        .unwrap();

    file.write(&HEADER).unwrap();

    led.set_high();

    let mut transfer = i2s_rx.read_dma_circular(&mut rx_buffer_i2s).unwrap();

    let mut data = [0u8; 2 * I2S_BYTES];

    // Ignore the first n buffers. Garbage data
    let mut ignore_counter = 6;

    async {
        loop {
            let avail = transfer.available().unwrap();

            if avail == 0 {
                continue;
            }

            transfer.pop(&mut data[..avail]).unwrap();

            if ignore_counter > 0 {
                ignore_counter -= 1;
                continue;
            }

            file.write(&data[..avail]).unwrap();
            file.flush().unwrap();

            if button.is_low() {
                // Debounce
                button.wait_for_high().await;
                break;
            }
        }

        log::info!("Done listening to mic!");
    }
    .await;

    let file_size: u32 = file.length();

    file.seek_from_start(4).unwrap();
    file.write(&(file_size - 8).to_ne_bytes()).unwrap();
    file.flush().unwrap();

    file.seek_from_start(40).unwrap();
    file.write(&(file_size - HEADER_SIZE as u32).to_ne_bytes())
        .unwrap();
    file.flush().unwrap();

    println!("File size: {}", file_size);
    file.close().unwrap();

    led.set_low();

    enter_deep_sleep(peripherals.GPIO3.degrade(), peripherals.LPWR);
}

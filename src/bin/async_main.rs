#![no_std]
#![no_main]

use core::u8;

use diane::http_date_time::MiniDateTime;
use diane::wifi_helper::WiFiHelper;
use diane::{enter_deep_sleep, setup_i2s, SDUtils, I2S_BYTES};
use embassy_executor::Spawner;
use embedded_sdmmc::Mode;
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, Pin, Pull};
use esp_hal::reset::wakeup_cause;
use esp_hal::rtc_cntl::{reset_reason, SocResetReason};
use esp_hal::Cpu;
use esp_hal::{
    gpio::{Level, Output},
    timer::systimer::SystemTimer,
};
use esp_hal_embassy::main;
use esp_println::println;
use log::info;

extern crate alloc;

const RECORDING_LOCATION: &str = "clips";

const SSID: &str = "Example";
const PASSWORD: &str = "password";

#[main]
async fn main(spawner: Spawner) {
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

    let mut button = Input::new(peripherals.GPIO10, Pull::Up);

    // -------------------
    // WiFi Setup
    // -------------------

    let wifi = WiFiHelper::new(
        spawner,
        peripherals.WIFI,
        peripherals.TIMG0,
        peripherals.RNG,
        peripherals.RADIO_CLK,
        SSID.try_into().unwrap(),
        PASSWORD.try_into().unwrap(),
    )
    .await;

    let (response, len) = wifi.http_get("motherfuckingwebsite.com").await;

    let (date, body) = if let Ok(response_str) = core::str::from_utf8(&response[..len]) {
        let date = response_str
            .lines()
            .find(|line| line.starts_with("date: "))
            .map(|line| line.trim_start_matches("date: ").trim())
            .unwrap_or_default();

        let body = response_str
            .find("\r\n\r\n")
            .map(|start| &response_str[start + 4..])
            .unwrap_or_default();

        (date, body)
    } else {
        ("", "")
    };

    let current_time = MiniDateTime::new(date);

    println!("Date: {}", current_time.sd_timestamp);
    println!("Body: {}", body);

    // -------------------
    // SD Card Setup
    // -------------------

    let mut volume_manager = SDUtils::setup_sd_card(
        peripherals.GPIO4.degrade(),
        peripherals.GPIO5.degrade(),
        peripherals.GPIO6.degrade(),
        peripherals.GPIO7.degrade(),
        peripherals.SPI2,
    );

    let mut volume = volume_manager
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .unwrap();
    println!("Volume 0: {:?}", volume);

    let mut root_dir = volume.open_root_dir().unwrap();

    // Create dir if it doesn't exist
    if root_dir.open_dir(RECORDING_LOCATION).is_err() {
        root_dir.make_dir_in_dir(RECORDING_LOCATION).unwrap();
    }

    let mut recordings = root_dir.open_dir(RECORDING_LOCATION).unwrap();

    let file_name = SDUtils::get_next_file_name(&mut recordings);

    let mut file = recordings
        .open_file_in_dir(file_name, Mode::ReadWriteCreate)
        .unwrap();

    SDUtils::write_wav_header(&mut file).unwrap();

    // -------------------
    // i2s Setup
    // -------------------

    let (mut i2s_rx, mut rx_buffer_i2s) = setup_i2s(
        peripherals.GPIO0.degrade(),
        peripherals.GPIO2.degrade(),
        peripherals.GPIO1.degrade(),
        peripherals.I2S0,
        peripherals.DMA_CH1,
    );

    let mut transfer = i2s_rx.read_dma_circular(&mut rx_buffer_i2s).unwrap();

    let mut data = [0u8; 2 * I2S_BYTES];

    // -------------------
    // Recording
    // -------------------

    async {
        // Ignore the first n buffers. Garbage data
        let mut ignore_counter = 6;

        loop {
            let avail = transfer.available().unwrap();

            if avail == 0 {
                continue;
            }

            transfer.pop(&mut data[..avail]).unwrap();

            if ignore_counter > 0 {
                ignore_counter -= 1;

                // Only turn on the LED once recording starts
                if ignore_counter == 0 {
                    led.set_high();
                }

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

    // -------------------
    // Cleanup
    // -------------------

    let file_size: u32 = file.length();

    SDUtils::update_wav_header(&mut file, file_size).unwrap();

    println!("File size: {}", file_size);
    file.close().unwrap();

    led.set_low();

    enter_deep_sleep(peripherals.GPIO3.degrade(), peripherals.LPWR);
}

#![no_std]
#![no_main]

use core::u8;

use diane::{enter_deep_sleep, setup_i2s, SDUtils, I2S_BYTES};
use embassy_executor::Spawner;
use embedded_sdmmc::Mode;
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

    let mut button = Input::new(peripherals.GPIO10, Pull::Up);

    // let timer1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    // let _init = esp_wifi::init(
    //     timer1.timer0,
    //     esp_hal::rng::Rng::new(peripherals.RNG),
    //     peripherals.RADIO_CLK,
    // )
    // .unwrap();

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

        led.set_high();

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

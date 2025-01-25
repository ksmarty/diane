#![no_std]
#![no_main]

use core::u8;

use aligned::A4;
use alloc::string::ToString;
use block_device_adapters::{BufStream, BufStreamError, StreamSlice};
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDeviceWithConfig;
use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_time::{Delay, Timer};
use embedded_fatfs::{FileSystem, FsOptions};
use embedded_io::SeekFrom;
use embedded_io_async::{Read, Seek, Write};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::gpio::{Input, Pull};
use esp_hal::i2s::master::{DataFormat, I2s, Standard};
use esp_hal::reset::wakeup_cause;
use esp_hal::rtc_cntl::sleep::{RtcioWakeupSource, WakeupLevel};
use esp_hal::rtc_cntl::{reset_reason, Rtc, SocResetReason};
use esp_hal::spi::master::SpiDmaBus;
use esp_hal::sync::RawMutex;
use esp_hal::{dma_buffers, gpio, Cpu};
use esp_hal::{
    gpio::{Level, Output},
    spi::master::{Config, Spi},
    time::RateExtU32,
    timer::systimer::SystemTimer,
};
use esp_hal_embassy::main;
use esp_println::println;
use log::info;
use mbr_nostd::{MasterBootRecord, PartitionTable};
use sdspi::{sd_init, SdSpi};

extern crate alloc;

// DMA buffer size
const I2S_BYTES: usize = 4092;

#[main]
async fn main(_spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let mut peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(72 * 1024);

    esp_println::logger::init_logger_from_env();

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    info!("Embassy initialized!");

    let reason = reset_reason(Cpu::ProCpu).unwrap_or(SocResetReason::ChipPowerOn);

    let wake_reason = wakeup_cause();

    info!("{:?}", reason);
    info!("{:?}", wake_reason);

    let mut led = Output::new(peripherals.GPIO9, Level::High);
    led.set_low();

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

    let dma_channel_spi = peripherals.DMA_CH0;
    let (rx_buffer_spi, rx_descriptors_spi, tx_buffer_spi, tx_descriptors_spi) =
        dma_buffers!(8 * I2S_BYTES);
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors_spi, rx_buffer_spi).unwrap();
    let dma_tx_buf = DmaTxBuf::new(tx_descriptors_spi, tx_buffer_spi).unwrap();

    let sclk = peripherals.GPIO4;
    let miso = peripherals.GPIO5;
    let mosi = peripherals.GPIO6;
    let mut cs = Output::new(peripherals.GPIO7, Level::Low);

    let spi = Spi::new(
        peripherals.SPI2,
        Config::default()
            .with_frequency(400.kHz())
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_mosi(mosi)
    .with_miso(miso)
    .with_dma(dma_channel_spi)
    .into_async();

    let mut spi = SpiDmaBus::new(spi, dma_rx_buf, dma_tx_buf);

    loop {
        match sd_init(&mut spi, &mut cs).await {
            Ok(_) => break,
            Err(e) => {
                log::warn!("Sd init error: {:?}", e);
                Timer::after_millis(10).await;
            }
        }
    }

    println!("sd_init complete");

    let spi_bus = Mutex::<RawMutex, _>::new(spi);
    let spid = SpiDeviceWithConfig::new(&spi_bus, cs, Config::default());
    let mut sd = SdSpi::<_, _, A4>::new(spid, Delay);

    while sd.init().await.is_err() {
        println!("Failed to init card, retrying...");
        Timer::after_millis(500).await;
    }
    // Increase the speed up to the SD max of 25mhz
    let mut config = Config::default();
    config.frequency = 25.MHz();
    sd.spi().set_config(config);

    let size = sd.size().await;
    println!("Initialization complete! Got card with size {:?}", size);

    // -------------------
    // i2s Setup
    // -------------------

    let bclk = peripherals.GPIO0; // sck - purple
    let din = peripherals.GPIO2; //  sd  - black
    let ws = peripherals.GPIO1; //   ws  - blue

    let dma_channel_i2s = peripherals.DMA_CH1;
    let (rx_buffer_i2s, rx_descriptors_i2s, _, tx_descriptors_i2s) = dma_buffers!(4 * I2S_BYTES, 0);

    let i2s = I2s::new(
        peripherals.I2S0,
        Standard::Philips,
        DataFormat::Data16Channel16,
        16.kHz(),
        dma_channel_i2s,
        rx_descriptors_i2s,
        tx_descriptors_i2s,
    )
    .into_async();

    let i2s_rx = i2s.i2s_rx.with_bclk(bclk).with_ws(ws).with_din(din).build();

    // https://github.com/AlexCharlton/mpfs-hal/blob/main/examples/src/bin/embassy-sd.rs

    let mut inner = BufStream::<_, 512>::new(sd);
    let mut buf = [0; 512];
    inner.read(&mut buf).await.unwrap();
    let mbr = MasterBootRecord::from_bytes(&buf).unwrap();
    println!("MBR: {:?}\n", mbr.partition_table_entries());

    let partition = mbr.partition_table_entries()[0];
    let start_offset = partition.logical_block_address as u64 * 512;
    let end_offset = start_offset + partition.sector_count as u64 * 512;
    let inner = StreamSlice::new(inner, start_offset, end_offset)
        .await
        .unwrap();

    let mut button = Input::new(peripherals.GPIO10, Pull::Up);

    async {
        let fs = FileSystem::new(inner, FsOptions::new()).await.unwrap();

        {
            let root_dir = fs.root_dir();
            let recordings = match root_dir.open_dir("recordings").await {
                Ok(v) => v,
                Err(_) => root_dir.create_dir("recordings").await.unwrap(),
            };
            let mut iter = recordings.iter();

            let mut highest_number = 0;
            while let Some(r) = iter.next().await {
                let e = r.unwrap();
                let name = e.file_name();
                let number = name
                    .split(".")
                    .nth(0)
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0);
                highest_number = highest_number.max(number);
            }

            let file_name = (highest_number + 1).to_string() + &".wav";

            let mut f = recordings.create_file(file_name.as_str()).await.unwrap();

            const HEADER_SIZE: usize = 44;
            let header: [u8; HEADER_SIZE] = [
                b'R', b'I', b'F', b'F', // ChunkID
                0xff, 0xff, 0xff, 0xff, // ChunkSize (to be filled)
                b'W', b'A', b'V', b'E', // Format
                b'f', b'm', b't', b' ', // Subchunk1ID
                16, 0, 0, 0, // Subchunk1Size (16 for PCM)
                1, 0, // AudioFormat (1 = PCM)
                2, 0, // NumChannels (1 = Mono)
                0x40, 0x1F, 0x00, 0x00, // SampleRate (8000 Hz)
                0x00, 0x7D, 0x00, 0x00, // ByteRate
                2, 0, // BlockAlign
                16, 0, // BitsPerSample
                b'd', b'a', b't', b'a', // Subchunk2ID
                0xff, 0xff, 0xff, 0xff, // Subchunk2Size (to be filled)
            ];
            f.write(&header).await.unwrap();
            f.flush().await.unwrap();

            led.set_high();

            let mut transfer = i2s_rx.read_dma_circular_async(rx_buffer_i2s).unwrap();
            let mut data = [0u8; I2S_BYTES];

            let mut total_data_bytes: u32 = 0;

            loop {
                let i2s_bytes_read = transfer.pop(&mut data).await.unwrap();
                total_data_bytes += i2s_bytes_read as u32;

                f.write(&data[..i2s_bytes_read]).await.unwrap();
                f.flush().await.unwrap();

                if button.is_low() {
                    // Debounce
                    button.wait_for_high().await;
                    break;
                }
            }

            let file_size = total_data_bytes + HEADER_SIZE as u32 - 8;
            f.seek(SeekFrom::Start(4)).await.unwrap();
            f.write(&(file_size).to_ne_bytes()).await.unwrap();

            f.seek(SeekFrom::Start(40)).await.unwrap();
            f.write(&(total_data_bytes).to_ne_bytes()).await.unwrap();

            println!(
                "File size: {} | Data bytes: {}",
                file_size, total_data_bytes
            );
        }

        fs.unmount().await.unwrap();

        Ok::<(), embedded_fatfs::Error<BufStreamError<sdspi::Error>>>(())
    }
    .await
    .expect("Filesystem tests failed!");

    log::info!("Out of the loop!");

    led.set_low();

    let wakeup_pins: &mut [(&mut dyn gpio::RtcPinWithResistors, WakeupLevel)] =
        &mut [(&mut peripherals.GPIO3, WakeupLevel::Low)];

    let wake_source = RtcioWakeupSource::new(wakeup_pins);

    let mut rtc = Rtc::new(peripherals.LPWR);
    rtc.sleep_deep(&[&wake_source])
}

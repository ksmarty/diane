#![no_std]
#![no_main]

use core::u8;

use aligned::A4;
use block_device_adapters::{BufStream, BufStreamError, StreamSlice};
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDeviceWithConfig;
use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_time::{Delay, Timer};
use embedded_fatfs::{FileSystem, FsOptions};
use embedded_io_async::{Read, Write};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::dma_buffers;
use esp_hal::i2s::master::{DataFormat, I2s, Standard};
use esp_hal::spi::master::SpiDmaBus;
use esp_hal::sync::RawMutex;
use esp_hal::{
    gpio::{Level, Output},
    spi::master::{Config, Spi},
    time::RateExtU32,
    timer::systimer::SystemTimer,
};
use esp_hal_embassy::main;
use esp_println::println;
use mbr_nostd::{MasterBootRecord, PartitionTable};
use sdspi::{sd_init, SdSpi};

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

    // -------------------
    // i2s Setup
    // -------------------

    let bclk = peripherals.GPIO0;
    let din = peripherals.GPIO2;
    let ws = peripherals.GPIO1;

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

    while let Err(err) = sd.init().await {
        println!("Failed to init card: {:?}, retrying...", err);
        Timer::after_millis(500).await;
    }
    // Increase the speed up to the SD max of 25mhz
    let mut config = Config::default();
    config.frequency = 25.MHz();
    sd.spi().set_config(config);

    let size = sd.size().await;
    println!("Initialization complete! Got card with size {:?}", size);

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

    async {
        let fs = FileSystem::new(inner, FsOptions::new()).await.unwrap();

        {
            let root_dir = fs.root_dir();

            let mut f = root_dir.create_file("example.wav").await.unwrap();

            let mut data = [0u8; I2S_BYTES];
            let mut transfer = i2s_rx.read_dma_circular_async(rx_buffer_i2s).unwrap();

            let mut iters = 0;

            loop {
                let i2s_bytes_read = transfer.pop(&mut data).await.unwrap();

                f.write(&data[..i2s_bytes_read]).await.unwrap();
                f.flush().await.unwrap();

                iters += 1;

                if iters > 100 {
                    break;
                }
            }
        }

        fs.unmount().await.unwrap();

        Ok::<(), embedded_fatfs::Error<BufStreamError<sdspi::Error>>>(())
    }
    .await
    .expect("Filesystem tests failed!");

    log::info!("Out of the loop!");
}

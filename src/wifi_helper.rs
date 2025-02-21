use alloc::format;
use embassy_executor::Spawner;
use embassy_net::dns::DnsQueryType;
use esp_embassy_wifihelper::WifiStack;
use esp_hal::peripheral::Peripheral;
use esp_hal::peripherals::{RADIO_CLK, RNG, TIMG0, WIFI};
use esp_println::println;
use log::info;

pub struct WiFiHelper {
    wifi: WifiStack,
}

impl WiFiHelper {
    pub async fn new(
        spawner: Spawner,
        wifi: impl Peripheral<P = WIFI> + 'static,
        timg0: impl Peripheral<P = TIMG0> + esp_hal::timer::timg::TimerGroupInstance,
        rng: impl Peripheral<P = RNG>,
        radio_clk: RADIO_CLK,
        ssid: &str,
        password: &str,
    ) -> WiFiHelper {
        let wifi = WifiStack::new(
            spawner,
            wifi,
            timg0,
            rng,
            radio_clk,
            ssid.try_into().unwrap(),
            password.try_into().unwrap(),
        );

        let config = wifi.wait_for_connected().await.unwrap();
        info!("Wifi connected with IP: {}", config.address);

        Self { wifi }
    }

    pub async fn http_get(&self, url: &str) -> ([u8; 1024], usize) {
        let res = *self
            .wifi
            .dns_query(url, DnsQueryType::A)
            .await
            .unwrap()
            .first()
            .unwrap();
        println!("dns: {:?}", res);

        let mut rx_buffer = [0u8; 1024];
        let mut tx_buffer = [0u8; 1024];

        let port = 80;

        let mut socket = self
            .wifi
            .make_and_connect_tcp_socket(res, port, &mut rx_buffer, &mut tx_buffer)
            .await
            .unwrap();

        let request = format!(
            "GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            url
        );
        socket.write(request.as_bytes()).await.unwrap();

        let mut response = [0u8; 1024];
        let len = socket.read(&mut response).await.unwrap();

        (response, len)
    }
}

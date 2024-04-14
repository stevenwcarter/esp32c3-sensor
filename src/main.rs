use anyhow::{bail, Result};
use core::str;
use dht_sensor::{dht22, Delay};
use embedded_svc::{
    http::{client::Client, Method},
    io::Read,
};
use esp32c3_sensor::wifi::wifi;
use esp_idf_svc::hal::gpio::{self, Gpio10, PinDriver};
use esp_idf_svc::hal::{delay, gpio::InputOutput};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::prelude::Peripherals,
    http::client::{Configuration, EspHttpConnection},
};
use serde::Serialize;
use std::thread;
use std::time::Duration;

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
}

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;

    let app_config = CONFIG;

    let _wifi = wifi(
        app_config.wifi_ssid,
        app_config.wifi_psk,
        peripherals.modem,
        sysloop,
    )?;

    let pin: gpio::Gpio10 = peripherals.pins.gpio10;
    let mut sensor = gpio::PinDriver::input_output_od(pin).unwrap();

    sensor.set_high().unwrap();

    let mut d = delay::Ets;

    loop {
        ReadingInput::take_reading(&mut d, &mut sensor);
        thread::sleep(Duration::from_millis(5 * 60 * 1000));
    }
}

#[derive(Serialize)]
pub struct ReadingInput {
    pub sensor: String,
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
    #[serde(rename = "heatIndex")]
    pub heat_index: Option<f64>,
}

impl ReadingInput {
    fn new(temp_c: f64, humidity: f64) -> Self {
        let temp_f = temp_c * 1.8 + 32.0;

        Self {
            sensor: "Bedroom".to_string(),
            temperature: Some(temp_f),
            humidity: Some(humidity),
            heat_index: None,
        }
    }
    fn take_reading(d: &mut impl Delay, sensor: &mut PinDriver<'_, Gpio10, InputOutput>) {
        match dht22::read(d, sensor) {
            Ok(r) => {
                println!(
                    "temperature: {}\tHumidity: {}",
                    r.temperature, r.relative_humidity
                );

                let reading = Self::new(r.temperature as f64, r.relative_humidity as f64);
                let _ = reading.send();
            }
            Err(e) => {
                println!("Failed with error: {:?}", e);
            }
        }
    }
    fn send(&self) -> Result<()> {
        let connection = EspHttpConnection::new(&Configuration {
            use_global_ca_store: true,
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        })?;
        // ANCHOR_END: connection
        let mut client = Client::wrap(connection);
        let body = serde_json::to_string(self).unwrap();

        // 2. Open a GET request to `url`
        let headers = [("accept", "application/json")];
        let mut request = client.request(
            Method::Post,
            "https://sensors.javapl.us/sensors/api/v1/sensor",
            &headers,
        )?;
        let _ = request.write(body.as_bytes());

        // 3. Submit write request and check the status code of the response.
        // Successful http status codes are in the 200..=299 range.
        let response = request.submit()?;
        let status = response.status();
        let location = response.header("location");
        if let Some(location) = location {
            println!("{location}");
        }

        println!("Response code: {}\n", status);

        match status {
            200..=299 => {
                // 4. if the status is OK, read response data chunk by chunk into a buffer and print it until done
                //
                // NB. see http_client.rs for an explanation of the offset mechanism for handling chunks that are
                // split in the middle of valid UTF-8 sequences. This case is encountered a lot with the given
                // example URL.
                let mut buf = [0_u8; 256];
                let mut offset = 0;
                let mut total = 0;
                let mut reader = response;
                loop {
                    if let Ok(size) = Read::read(&mut reader, &mut buf[offset..]) {
                        if size == 0 {
                            break;
                        }
                        total += size;
                        // 5. try converting the bytes into a Rust (UTF-8) string and print it
                        let size_plus_offset = size + offset;
                        match str::from_utf8(&buf[..size_plus_offset]) {
                            Ok(text) => {
                                print!("{}", text);
                                offset = 0;
                            }
                            Err(error) => {
                                let valid_up_to = error.valid_up_to();
                                unsafe {
                                    print!("{}", str::from_utf8_unchecked(&buf[..valid_up_to]));
                                }
                                buf.copy_within(valid_up_to.., 0);
                                offset = size_plus_offset - valid_up_to;
                            }
                        }
                    }
                }
                println!("Total: {} bytes", total);
            }
            _ => bail!("Unexpected response code: {}", status),
        }
        Ok(())
    }
}

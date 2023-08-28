#![allow(clippy::type_complexity, clippy::unit_arg)]

use anyhow::Result;
use esp_idf_hal::{delay::Delay, gpio::*, peripherals::Peripherals, task::executor};
use simple_logger::SimpleLogger;

fn main() {
    match main_() {
        Ok(()) => log::info!("main returned Ok"),
        Err(e) => log::error!("main returned Err: {e}"),
    }
    restart()
}

fn main_() -> Result<()> {
    SimpleLogger::new().init().unwrap();
    esp_idf_sys::link_patches(); // hack to make sure that a few patches are linked into the final executable

    log::info!("Good morning, let's bake this bread!");

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;

    // let mut white_led = PinDriver::output(pins.gpio19)?;
    // white_led.set_low()?;
    let mut yellow_led = PinDriver::output(pins.gpio18)?;
    yellow_led.set_low()?;

    let mut encoder_button = PinDriver::input(pins.gpio1)?;
    encoder_button.set_pull(Pull::Up)?;

    executor::EspBlocker::new().block_on(main_loop(&mut yellow_led, &mut encoder_button))?;

    Ok(())
}

async fn main_loop(
    yellow_led: &mut PinDriver<'_, impl OutputPin, Output>,
    encoder_button: &mut PinDriver<'_, impl InputPin, Input>,
) -> Result<()> {
    let mut led_on_state = false;
    loop {
        encoder_button.wait_for_any_edge(false).await?;
        led_on_state = encoder_button.is_high();
        if led_on_state {
            yellow_led.set_low()?;
        } else {
            yellow_led.set_high()?;
        }
    }
}

fn restart() -> ! {
    log::info!("restarting");
    esp_idf_hal::reset::restart();
    std::process::abort()
}

#![allow(clippy::type_complexity, clippy::unit_arg)]

use anyhow::Result;
use esp_idf_hal::{
    gpio::*,
    ledc::{config::TimerConfig, LedcDriver, LedcTimerDriver},
    peripherals::Peripherals,
    prelude::*,
    task::{executor, watchdog},
};
use palette::{FromColor, Hsv, Srgb};
use simple_logger::SimpleLogger;
use std::mem::replace;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INPUT_GHOST_COOLDOWN: Duration = Duration::from_millis(40);

const TAP_TIMEOUT: Duration = Duration::from_millis(240);

#[derive(Clone, Copy)]
enum KeySt {
    Pressed(Instant),
    Released(Instant),
    Free,
}

static mut ENCODER_BUTTON: KeySt = KeySt::Free;

// State needed to decode the gray code of encoder with bouncing in the signals.
// Same problem as
// https://hackaday.com/2022/04/20/a-rotary-encoder-how-hard-can-it-be/
// but different solution.
struct EncoderSt {
    edge_was_a: bool,
    a_was_high: bool,
    b_was_high: bool,
    cw_updates: i8,
}

static mut ENCODER_ST: EncoderSt = EncoderSt { edge_was_a: true, a_was_high: true, b_was_high: true, cw_updates: 0 };

fn main() {
    match executor::EspBlocker::new().block_on(main_()) {
        Ok(()) => log::info!("main returned Ok"),
        Err(e) => log::error!("main returned Err: {e}"),
    }
    restart()
}

async fn main_() -> Result<()> {
    SimpleLogger::new().init().unwrap();
    esp_idf_sys::link_patches(); // hack to make sure that a few patches are linked into the final executable

    log::info!("Good morning, let's bake this bread!");

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;

    let mut wdog_driver =
        esp_idf_hal::task::watchdog::TWDTDriver::new(peripherals.twdt, &watchdog::config::Config::default())?;
    let mut wdog = wdog_driver.watch_current_task()?;

    // let mut white_led = PinDriver::output(pins.gpio19)?;
    // white_led.set_low()?;
    let mut yellow_led = PinDriver::output(pins.gpio18)?;
    yellow_led.set_low()?;

    let mut encoder_button = PinDriver::input(pins.gpio1)?;
    encoder_button.set_pull(Pull::Up)?;
    unsafe {
        encoder_button.subscribe(|| press_encoder())?;
    }
    encoder_button.set_interrupt_type(InterruptType::AnyEdge)?;
    encoder_button.enable_interrupt()?;

    let encoder_a = Arc::new(Mutex::new(PinDriver::input(pins.gpio2)?));
    let encoder_b = Arc::new(Mutex::new(PinDriver::input(pins.gpio6)?));
    let encoder_a1 = encoder_a.clone();
    let encoder_b1 = encoder_b.clone();
    {
        let mut encoder_a = encoder_a.lock().unwrap();
        let mut encoder_b = encoder_b.lock().unwrap();
        encoder_a.set_pull(Pull::Up)?;
        encoder_b.set_pull(Pull::Up)?;
        unsafe {
            encoder_a.subscribe(move || {
                let a_is_high = encoder_a1.lock().unwrap().is_high();

                if !ENCODER_ST.edge_was_a && !ENCODER_ST.b_was_high && ENCODER_ST.a_was_high {
                    ENCODER_ST.cw_updates = ENCODER_ST.cw_updates.saturating_sub(1);
                }

                ENCODER_ST.a_was_high = a_is_high;
                ENCODER_ST.edge_was_a = true;
            })?;
            encoder_b.subscribe(move || {
                let b_is_high = encoder_b1.lock().unwrap().is_high();

                if ENCODER_ST.edge_was_a && !ENCODER_ST.a_was_high && ENCODER_ST.b_was_high {
                    ENCODER_ST.cw_updates = ENCODER_ST.cw_updates.saturating_add(1);
                }

                ENCODER_ST.b_was_high = b_is_high;
                ENCODER_ST.edge_was_a = false;
            })?;
        }
        encoder_a.set_interrupt_type(InterruptType::AnyEdge)?;
        encoder_b.set_interrupt_type(InterruptType::AnyEdge)?;
        encoder_a.enable_interrupt()?;
        encoder_b.enable_interrupt()?;
    }

    let (red_led, green_led, blue_led) = (pins.gpio3, pins.gpio4, pins.gpio5);
    let config = TimerConfig::default().frequency(10.kHz().into());
    let timer0 = LedcTimerDriver::new(peripherals.ledc.timer0, &config)?;
    let timer1 = LedcTimerDriver::new(peripherals.ledc.timer1, &config)?;
    let timer2 = LedcTimerDriver::new(peripherals.ledc.timer2, &config)?;
    let mut channel0 = LedcDriver::new(peripherals.ledc.channel0, &timer0, red_led)?;
    let mut channel1 = LedcDriver::new(peripherals.ledc.channel1, &timer1, green_led)?;
    let mut channel2 = LedcDriver::new(peripherals.ledc.channel2, &timer2, blue_led)?;
    let max_duty0 = channel0.get_max_duty();
    let max_duty1 = channel1.get_max_duty();
    let max_duty2 = channel2.get_max_duty();

    let mut t = Instant::now();
    let dt_min = Duration::from_millis(33);
    std::thread::sleep(dt_min);
    let mut led_on = true;
    let mut led_color = Hsv::new(0.0, 1.0, 0.3);
    loop {
        let dt = t.elapsed();
        let dtf = dt.as_secs_f32();
        t = Instant::now();

        let (mut encoder_tapped, mut encoder_held) = (false, false);
        match get_encoder_key_st() {
            KeySt::Pressed(tp) if encoder_button.is_high() => {
                if (t - tp) < TAP_TIMEOUT {
                    encoder_tapped = true
                }
                set_encoder_key_st(KeySt::Released(t))
            }
            KeySt::Pressed(tp) if (t - tp) > TAP_TIMEOUT => encoder_held = true,
            KeySt::Released(tr) if (t - tr) > INPUT_GHOST_COOLDOWN => set_encoder_key_st(KeySt::Free),
            _ => (),
        }

        let encoder_cw = unsafe { replace(&mut ENCODER_ST.cw_updates, 0) };

        if encoder_tapped {
            led_on ^= true;
        }
        if encoder_held {
            led_color.hue += dtf * 50.0;
        }
        led_color.value = (led_color.value + encoder_cw as f32 * 0.1).clamp(0.0, 1.0);

        let mut color = if led_on { led_color } else { Hsv::new(0.0, 0.0, 0.0) };
        color.value = color.value.powf(2.4);
        let (r, g, b) = Srgb::from_color(color).into_components();
        channel0.set_duty((max_duty0 as f32 * r) as u32)?;
        channel1.set_duty((max_duty1 as f32 * g) as u32)?;
        channel2.set_duty((max_duty2 as f32 * b) as u32)?;

        wdog.feed()?;
        std::thread::sleep(dt_min.saturating_sub(dt));
    }
}

fn get_encoder_key_st() -> KeySt {
    unsafe { ENCODER_BUTTON }
}
fn set_encoder_key_st(st: KeySt) {
    unsafe { ENCODER_BUTTON = st }
}
fn press_encoder() {
    if let KeySt::Free = get_encoder_key_st() {
        set_encoder_key_st(KeySt::Pressed(Instant::now()))
    }
}

fn restart() -> ! {
    log::info!("restarting");
    esp_idf_hal::reset::restart();
    std::process::abort()
}

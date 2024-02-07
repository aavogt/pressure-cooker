#![feature(generic_const_exprs)]
use std::{thread, sync::{atomic::AtomicU8, Arc}, rc::Rc, cell::{RefCell, Cell}, iter::zip, ffi::c_void};

use ds18b20::Ds18b20;
use embedded_graphics::{primitives::{Polyline, PrimitiveStyle, Primitive}, prelude::Point, pixelcolor::BinaryColor, mono_font::{ascii::FONT_6X10, MonoTextStyleBuilder}, text::Text, Drawable};
use esp_idf_hal::{prelude::Peripherals, delay::{FreeRtos, Ets}, gpio::{PinDriver, AnyIOPin}, i2c::{config::Config, I2cDriver}, units::Hertz};
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use anyhow::anyhow;
use one_wire_bus::OneWire;
use ssd1306::{rotation::DisplayRotation, size::DisplaySize128x32, Ssd1306, I2CDisplayInterface, prelude::{DisplayConfig, I2CInterface}, mode::BufferedGraphicsMode};
use std::sync::atomic::Ordering::Relaxed;
use log::*;

use itertools::Itertools;

use circular_buffer::*;


// TODO hardware gets stuck see README.md
// TODO name the states ie. enum instead of u8
fn mk_buzzer(pin : AnyIOPin) -> anyhow::Result<Box<dyn FnMut(u8)>> {
    let mut buzzer = PinDriver::output(pin)?;
    let buzzing = Arc::new(AtomicU8::new(0));
    let buzzing1 = buzzing.clone();
    info!("starting buzzer thread");
    thread::spawn(move || {
        let mut on_off = |high,low| {
            if high != 0 {
                buzzer.set_high().unwrap();
                FreeRtos::delay_ms(high);
            };
            if low != 0 {
                buzzer.set_low().unwrap();
                FreeRtos::delay_ms(low);
            }
        };

        loop {
            // check atomicU8 whether to buzz urgently, buzz, or be quiet
            match buzzing1.load(Relaxed) {
                2 => {
                    on_off(400, 400);
                },
                1 => {
                    on_off(400, 400);
                    FreeRtos::delay_ms(1000);
                },
                _ => {
                    FreeRtos::delay_ms(1000);
                },
            }
        }
    });
    Ok(Box::new(move |n| buzzing.store(n, Relaxed)))
}

// this must surely be a library function, but into() needs a type annotation
fn result_to_either<T, E>(x: Result<T, E>) -> itertools::Either<T, E> {
    match x {
        Ok(v) => itertools::Either::Left(v),
        Err(e) => itertools::Either::Right(e),
    }
}

// I want to change this to get temperatures for all of the sensors,
// sorted by address, should these addresses be returned?
fn mk_get_temp(pin : AnyIOPin) -> anyhow::Result<Box<dyn FnMut() -> anyhow::Result<Vec<f32>>>> {

    let pindriver = PinDriver::input_output_od(pin)?;
    let mut one_wire_bus = OneWire::new(pindriver).map_err(|_| anyhow!("Failed to initialize 1-wire bus"))?;

    let f = move || -> anyhow::Result<Vec<f32>> {

        let (mut addrs,errs) : (Vec<_>, Vec<_>) = one_wire_bus.devices(false, &mut Ets)
              .partition(|x| x.is_ok());

        addrs.sort_by_key(|a| a.unwrap().0);

        let reads : Vec<_> = addrs.into_iter().map(|addr| {
          let dev = Ds18b20::new::<anyhow::Error>(addr.unwrap())
              .map_err(|x| anyhow!("onewire can't init ds18b20 {:?}", x))?;

          info!("addr: {:?}", addr);
          dev.start_temp_measurement(
              &mut one_wire_bus,
              &mut Ets)
              .map_err(|x| anyhow!("onewire can't start measurment {:?}", x))?;
          dev.read_data(
              &mut one_wire_bus,
              &mut Ets)
              .map_err(|x| anyhow!("onewire can't finish measurment {:?}", x))
              .map(|x| x.temperature)
        }).filter_map(|x| x.ok()).collect();

        Ok(reads)
    };
    Ok(Box::new(f))
}


fn mk_display<'d>(i2c_driver : I2cDriver<'d>) ->
    anyhow::Result<Ssd1306<I2CInterface<I2cDriver<'d>>,DisplaySize128x32,BufferedGraphicsMode<DisplaySize128x32>>> {
    let i2c_interface = I2CDisplayInterface::new(i2c_driver);
    let mut display = Ssd1306::new(i2c_interface, DisplaySize128x32, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().map_err(|x| anyhow!("display.init() error: {:?}", x))?;
    Ok(display)
}

const W_TEXT : usize = 32;

fn main() -> anyhow::Result<()>{
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    // Initialize the logger
    esp_idf_svc::log::EspLogger::initialize_default();
    std::env::set_var("RUST_BACKTRACE", "1");


    info!("initializing peripherals");
    let peripherals = Peripherals::take()?;

    let mut state: State<{128-W_TEXT}> = State::new();

    let mut get_temp = mk_get_temp(AnyIOPin::from(peripherals.pins.gpio13))?;

    info!("initializing i2c display");
    let i2c_config = Config::new().baudrate(Hertz(1_000_000));
    let i2c_driver = I2cDriver::new(peripherals.i2c0,
                                    peripherals.pins.gpio3,
                                    peripherals.pins.gpio2,
                                    &i2c_config)?;
    let mut display = mk_display(i2c_driver)?;
    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    info!("initializing buzzer");
    let _set_buzz = mk_buzzer(AnyIOPin::from(peripherals.pins.gpio0))?;

    info!("starting temperature/display thread");
    // loop to read temperature and display it,
    let times : CircularBuffer<u128, 2> = CircularBuffer::new();
    loop {
        // take a temperature measurement
        let temperatures = get_temp()?;
        let temperature = temperatures[0];
        info!("TEMPS, {}", temperatures.into_iter().map(|x| x.to_string()).join(","));
        state.push(temperature);
        state.push_time();

        let mut rounded_at = |n : f32, y|
            Text::new(&format!("{:3}", (n * 10.0).round() / 10.0), Point{ x : 0, y }, text_style)
            .draw(&mut display)
            .map_err(|x| anyhow!("display error {:?}", x));

        // on the left side draw the temperature bounds
        // at the top and bottom, and the current temperature in the middle
        rounded_at(temperature, 19)?;
        rounded_at(state.t_high, 7)?;
        rounded_at(state.t_low, 31)?;

        // draw the temperature graph to the right of the numbers
        Polyline::new(state.past_points.as_slice())
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(&mut display)
            .map_err(|x| anyhow!("display error {:?}", x))?;

        info!("displaying");
        display.flush().map_err(|x| anyhow!("display error {:?}", x))?;

        state.push_time();


        let wait = 625 - state.time_delta().unwrap_or(0) as i64;
        if wait > 0 { 
            info!("waiting {}ms", wait);
            FreeRtos::delay_ms(wait as u32);
        };

        display.clear();
    }
}



// the state used to draw the temperature graph
// whose bounds adjust to fit
// TODO "times" could be longer to account for time spent setting up the delay
struct State<const N: usize> where [(); 2*N]: {
   past_temperatures : CircularBuffer<f32, N>,
   past_points : CircularBuffer<Point, N>,
   times : CircularBuffer<u128, 2>,
   t_low : f32,
   t_high : f32,
}

impl<const N : usize> State<N> where [(); 2*N]: {
    fn new() -> Self {
        Self {
            past_temperatures : CircularBuffer::new(),
            past_points : CircularBuffer::new(),
            times : CircularBuffer::<_,2>::new(),
            t_low : 20.0,
            t_high : 25.0,
        }
    }

    fn time_delta(&self) -> Option<u128> {
        Some(self.times.head()? - self.times.last()?)
    }

    fn push_time(&mut self) {
        self.times.push(EspSystemTime.now().as_millis());
    }

    /// add a new temperature to the state
    /// and update the temperature bounds
    /// and the points to draw
    /// somewhat buggy: it can cut off a recent peak
    /// when the oldest temperatures are nealy identical
    fn push(&mut self, temp : f32) {
        let mut dirty = false;
        let dropped = self.past_temperatures.last();
        let was_full = self.past_temperatures.is_full();
        self.past_temperatures.push(temp);

        // expand temperature bounds
        if temp < self.t_low {
            self.t_low = temp;
            dirty = true;
        }
        if temp > self.t_low {
            self.t_high = temp;
            dirty = true;
        }

        let may_drop_bound_temp = was_full && [self.t_low, self.t_high].iter()
                .any(|&x| dropped.map(|o| (x-o).abs() < 0.1).unwrap_or(false) );

        // recalculate temp_bounds
        if may_drop_bound_temp {
            let mut iter = self.past_temperatures.into_iter();
            if let Some(t0) = iter.next() {
                self.t_low = t0-0.05;
                self.t_high = t0+0.05;
                for t in iter {
                    if t < self.t_low {
                        self.t_low = t;
                    }
                    if t > self.t_high {
                        self.t_high = t;
                    }
                }
            }
        }

        // update past_points to account for the new bounds
        let scale = |x| (31.0 - 30.0 * (x - self.t_low) / (self.t_high - self.t_low)) as i32;

        // couldn't get std::iter::zip to do this
        // because mutating the equivalent of p there didn't change the value in past_points
        self.past_points.zip_with(&self.past_temperatures, |p, &t| {
            p.x+=1; // shift to the right
            if dirty { // when temp_bounds changed, recalculate y
                p.y = scale(t);
        }});

        // add the newest point
        self.past_points.push(Point{ x: W_TEXT as i32, y: scale(temp) });
    }
}


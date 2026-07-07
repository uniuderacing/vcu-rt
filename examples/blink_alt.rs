//! A simple VCU whose only task is to blink an LED.
//! The `_alt` suffix in the name is needed to avoid a collision with the other `blink` example in the root crate of the
//! workspace.

use core::time::Duration;
use esp_idf_hal::{gpio, prelude::*};
use vcu_rt::prelude::*;

/// Representation of the action that blinks the LED.
struct BlinkAction<P: gpio::OutputPin> {
    /// Output pin where the LED is connected to.
    led: gpio::PinDriver<'static, P, gpio::Output>,
}

impl<P: gpio::OutputPin> BlinkAction<P> {
    /// Constructs a new instance of [`BlinkAction`].
    fn new(pin: P) -> Self {
        Self {
            led: gpio::PinDriver::output(pin).unwrap(),
        }
    }
}

#[async_trait]
impl<P: gpio::OutputPin> vcu_rt::task::IAction for BlinkAction<P> {
    type Input = ();

    async fn init(&mut self, _vcu_resources: vcu_rt::param::VcuResources) {}

    async fn run(&mut self, _input: Self::Input) {
        self.led.toggle().unwrap();
    }
}

fn main() {
    vcu_rt::hal::init(false).unwrap();
    let peripherals = Peripherals::take().unwrap();

    // Board LEDs pinout:
    // - 2025 common block: GPIO48 (LED1), GPIO47 (LED2)

    // Create blink task
    let timer = Duration::from_millis(500);
    let blink_action = BlinkAction::new(peripherals.pins.gpio48);
    let blink_task = vcu_rt::task::Recurrent::new("Blink", false, Some(timer), blink_action);

    // Build VCU runtime
    let mut vcu_rt = vcu_rt::Runtime::builder()
        .with_vcu_id(0x00)
        .with_vcu_name("Example")
        .with_task(blink_task)
        .build()
        .unwrap();

    // Start VCU runtime to run all tasks
    vcu_rt.start();
}

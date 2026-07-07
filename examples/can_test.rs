//! A simple VCU that can be used to test CAN bus operations.

use esp_idf_hal::{can, prelude::*};
use vcu_rt::prelude::*;

/// Representation of the action that logs received ping messages.
struct PingLoggerAction;

impl PingLoggerAction {
    /// Constructs a new instance of [`PingLoggerAction`].
    const fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl vcu_rt::task::IAction for PingLoggerAction {
    type Input = serena_messages::PingMessage;

    async fn init(&mut self, _vcu_resources: vcu_rt::param::VcuResources) {}

    async fn run(&mut self, input: Self::Input) {
        log::info!("Received ping from VCU with ID {:#04x}", input.board_id());
    }
}

fn main() {
    vcu_rt::hal::init(false).unwrap();
    let peripherals = Peripherals::take().unwrap();

    // Initialize CAN bus driver
    let can_config = can::config::Config::new()
        .filter(can::config::Filter::standard_allow_all())
        .timing(can::config::Timing::B500K)
        .mode(can::config::Mode::Normal)
        .rx_queue_len(4096)
        .tx_queue_len(64);
    let can_driver = can::CanDriver::new(
        peripherals.can,
        peripherals.pins.gpio14,
        peripherals.pins.gpio13,
        &can_config,
    )
    .unwrap();

    // Create ping logger task
    let logger_action = PingLoggerAction::new();
    let logger_task = vcu_rt::task::CanMessage::new("Ping logger", false, logger_action);

    // Build VCU runtime
    let mut vcu_rt = vcu_rt::Runtime::builder()
        .with_vcu_id(0x00)
        .with_vcu_name("Example")
        .with_can_driver(can_driver, None, false)
        .with_board_status(false)
        .with_tacom_ota(false)
        .with_task(logger_task)
        .build()
        .unwrap();

    // Start VCU runtime to run all tasks
    vcu_rt.start();
}

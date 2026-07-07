//! A simple VCU that can be used to monitor the load of the CAN bus.

use core::time::Duration;
use esp_idf_hal::{can, prelude::*};
use vcu_rt::prelude::*;

/// Representation of the action that monitors the CAN bus load.
struct MonitorAction {
    /// Set of resources that can be used to interact with the VCU hardware.
    vcu_resources: Option<vcu_rt::param::VcuResources>,
}

impl MonitorAction {
    /// Constructs a new instance of [`MonitorAction`].
    const fn new() -> Self {
        Self {
            vcu_resources: None,
        }
    }
}

#[async_trait]
impl vcu_rt::task::IAction for MonitorAction {
    type Input = ();

    async fn init(&mut self, vcu_resources: vcu_rt::param::VcuResources) {
        self.vcu_resources = Some(vcu_resources);
    }

    async fn run(&mut self, _input: Self::Input) {
        log::info!(
            "Current CAN bus load: {:.2}%",
            self.vcu_resources
                .as_ref()
                .unwrap()
                .can_bus
                .as_ref()
                .unwrap()
                .load
                .percentage(can::config::Timing::B500K)
        );
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

    // Create monitoring task
    let monitor_action = MonitorAction::new();
    let monitor_task = vcu_rt::task::Recurrent::new(
        "CAN bus load monitor",
        false,
        Some(Duration::from_secs(1)),
        monitor_action,
    );

    // Build VCU runtime
    let mut vcu_rt = vcu_rt::Runtime::builder()
        .with_vcu_id(0x00)
        .with_vcu_name("Example")
        .with_can_driver(can_driver, None, false)
        .with_board_status(false)
        .with_tacom_ota(false)
        .with_tacom_ping(false)
        .with_task(monitor_task)
        .build()
        .unwrap();

    // Start VCU runtime to run all tasks
    vcu_rt.start();
}

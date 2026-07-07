//! Task that periodically sends board status messages.

use crate::{
    config,
    param::{PowerConsumption, VcuResources},
    task::{IAction, ITask, Recurrent},
};
use async_trait::async_trait;

/// Returns a new instance of the status task.
pub fn status(vcu_id: u8) -> impl ITask {
    let action = Status::new(vcu_id);
    Recurrent::new(
        "Status",
        false,
        Some(config::can::STATUS_REPORT_INTERVAL),
        action,
    )
}

// +--------+
// | Action |
// +--------+

/// Representation of the action that sends board status messages.
pub struct Status {
    /// Unique ID of the VCU.
    vcu_id: u8,
    /// Set of resources that can be used to interact with the VCU hardware.
    vcu_resources: Option<VcuResources>,
}

impl Status {
    /// Constructs a new instance of [`Status`].
    const fn new(vcu_id: u8) -> Self {
        Self {
            vcu_id,
            vcu_resources: None,
        }
    }
}

#[async_trait]
impl IAction for Status {
    type Input = ();

    async fn init(&mut self, vcu_resources: VcuResources) {
        // This method is called only if the CAN bus is managed by the VCU runtime, therefore the resources must exist
        std::assert!(vcu_resources.can_bus.is_some());
        self.vcu_resources = Some(vcu_resources);
    }

    async fn run(&mut self, _input: Self::Input) {
        let power_consumption = self
            .vcu_resources
            .as_ref()
            .unwrap()
            .power_consumption
            .as_ref()
            .map_or(0.0, PowerConsumption::milliamps);
        let message = serena_messages::BoardStatusMessage::new()
            .with_board_id(self.vcu_id)
            .with_power_consumption(power_consumption.try_into().unwrap_or_else(|err| {
                log::error!("Failed to convert power consumption ({power_consumption:.3}mA) into message: {err}");
                0.0.try_into().unwrap()
            }));
        self.vcu_resources
            .as_ref()
            .unwrap()
            .can_bus
            .as_ref()
            .unwrap()
            .stream
            .send_async(message)
            .await;
    }
}

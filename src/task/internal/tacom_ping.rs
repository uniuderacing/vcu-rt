//! Task that periodically sends Tacom pings.

use crate::{
    config,
    param::VcuResources,
    task::{IAction, ITask, Recurrent},
};
use async_trait::async_trait;

/// Returns a new instance of the Tacom ping task.
pub fn tacom_ping(vcu_id: u8) -> impl ITask {
    let action = TacomPing::new(vcu_id);
    Recurrent::new(
        "Tacom ping",
        false,
        Some(config::tacom::PING_INTERVAL),
        action,
    )
}

// +--------+
// | Action |
// +--------+

/// Representation of the action that sends Tacom pings.
#[allow(clippy::module_name_repetitions)]
pub struct TacomPing {
    /// Unique ID of the VCU.
    vcu_id: u8,
    /// Set of resources that can be used to interact with the VCU hardware.
    vcu_resources: Option<VcuResources>,
}

impl TacomPing {
    /// Constructs a new instance of [`TacomPing`].
    const fn new(vcu_id: u8) -> Self {
        Self {
            vcu_id,
            vcu_resources: None,
        }
    }
}

#[async_trait]
impl IAction for TacomPing {
    type Input = ();

    async fn init(&mut self, vcu_resources: VcuResources) {
        // This method is called only if the CAN bus is managed by the VCU runtime, therefore the resources must exist
        std::assert!(vcu_resources.can_bus.is_some());
        self.vcu_resources = Some(vcu_resources);
    }

    async fn run(&mut self, _input: Self::Input) {
        let message = serena_messages::PingMessage::new().with_board_id(self.vcu_id);
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

//! Task that periodically reads the value sent by the eFuse to monitor the VCU power consumption.

use crate::{
    config,
    param::{AsyncAdc, PowerConsumption, VcuResources},
    task::{IAction, ITask, Recurrent},
};
use async_trait::async_trait;
use portable_atomic::AtomicF32;
use std::sync::{atomic::Ordering, Arc};

/// Returns a new instance of the power monitor task, alongside a shared object that can be used by other tasks to
/// retrieve the read voltage.
pub fn power_monitor(
    efuse_adc: Box<dyn AsyncAdc + Send>,
    efuse_constants: (f64, f64),
) -> (impl ITask, PowerConsumption) {
    let v_ilm = Arc::new(AtomicF32::new(0.0));
    let action = PowerMonitor::new(efuse_adc, v_ilm.clone());
    (
        Recurrent::new(
            "Power monitor",
            false,
            Some(config::hw::EFUSE_READ_INTERVAL),
            action,
        ),
        PowerConsumption::new(v_ilm, efuse_constants),
    )
}

// +--------+
// | Action |
// +--------+

/// Representation of the action that reads the voltage signal from the eFuse.
pub struct PowerMonitor {
    /// Hardware interface with the eFuse.
    efuse_adc: Box<dyn AsyncAdc + Send>,
    /// Shared reference to the current measured voltage from the eFuse.
    v_ilm: Arc<AtomicF32>,
    /// Set of resources that can be used to interact with the VCU hardware.
    vcu_resources: Option<VcuResources>,
}

impl PowerMonitor {
    /// Constructs a new instance of [`PowerMonitor`].
    const fn new(efuse_adc: Box<dyn AsyncAdc + Send>, v_ilm: Arc<AtomicF32>) -> Self {
        Self {
            efuse_adc,
            v_ilm,
            vcu_resources: None,
        }
    }
}

#[async_trait]
impl IAction for PowerMonitor {
    type Input = ();

    async fn init(&mut self, vcu_resources: VcuResources) {
        // This method is called only if the eFuse is managed by the VCU runtime, therefore the resource must exist
        std::assert!(vcu_resources.power_consumption.is_some());
        self.vcu_resources = Some(vcu_resources);
    }

    async fn run(&mut self, _input: Self::Input) {
        let mut avg = 0.0;
        for _ in 0..config::hw::EFUSE_READ_NUM_SAMPLES {
            let sample = match self.efuse_adc.read().await {
                Ok(value) => value,
                Err(err) => {
                    log::error!("Failed to read eFuse power consumption feedback voltage: {err}");
                    return;
                }
            };
            avg += sample;
        }

        let v_ilm = avg / f32::from(config::hw::EFUSE_READ_NUM_SAMPLES);
        self.v_ilm.store(v_ilm, Ordering::SeqCst);
    }
}

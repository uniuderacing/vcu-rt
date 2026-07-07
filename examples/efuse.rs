//! A simple VCU that can be used to monitor its power consumption, as reported byan eFuse.

use core::time::Duration;
use esp_idf_hal::{adc, prelude::*};
use std::sync::Arc;
use vcu_rt::prelude::*;

/// Representation of the action that logs the VCU power consumption.
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
            "Current power consumption: {:.3}mA",
            self.vcu_resources
                .as_ref()
                .unwrap()
                .power_consumption
                .as_ref()
                .unwrap()
                .milliamps()
        );
    }
}

fn main() {
    vcu_rt::hal::init(false).unwrap();
    let peripherals = Peripherals::take().unwrap();

    // eFuse pinout:
    // - 2025 common block: GPIO2 (ADC1)

    // Initialize eFuse ADC
    let adc_driver = adc::oneshot::AdcDriver::new(peripherals.adc1).unwrap();
    let shared_adc_driver = Arc::new(adc_driver);
    let adc_channel_config = adc::oneshot::config::AdcChannelConfig {
        attenuation: adc::attenuation::DB_11,
        resolution: adc::Resolution::Resolution12Bit,
        calibration: adc::oneshot::config::Calibration::Curve,
    };
    let adc_channel_driver = adc::oneshot::AdcChannelDriver::new(
        shared_adc_driver.clone(),
        peripherals.pins.gpio2,
        &adc_channel_config,
    )
    .unwrap();
    let efuse_adc = vcu_rt::param::EspAdc::new(adc_channel_driver);

    // Create monitor task
    let timer = Duration::from_secs(1);
    let monitor_action = MonitorAction::new();
    let monitor_task = vcu_rt::task::Recurrent::new(
        "Power consumption monitor",
        false,
        Some(timer),
        monitor_action,
    );

    // Build VCU runtime
    let mut vcu_rt = vcu_rt::Runtime::builder()
        .with_vcu_id(0x00)
        .with_vcu_name("Example")
        .with_efuse(efuse_adc, None, None)
        .with_task(monitor_task)
        .build()
        .unwrap();

    // Start VCU runtime to run all tasks
    vcu_rt.start();
}

//! Centralized configuration options.

/// Hardware configuration.
pub mod hw {
    use core::time::Duration;

    /// Default value of the constant `G_imon` for the VCU eFuses.
    pub const EFUSE_DEFAULT_G_IMON: f64 = 657.15 * 1e-6;
    /// Default value of the constant `R_ilm` for the VCU eFuses.
    pub const EFUSE_DEFAULT_R_ILM: f64 = 2.0 * 1e3; // In ohm
    /// How many samples to collect and average to reduce noise in eFuse voltage readings.
    /// Reference:
    /// <https://docs.espressif.com/projects/esp-idf/en/v5.2.3/esp32/api-reference/peripherals/adc_calibration.html#minimize-noise>.
    pub const EFUSE_READ_NUM_SAMPLES: u16 = 8;
    /// Rate at which to read VCU power consumption values from the eFuse.
    pub const EFUSE_READ_INTERVAL: Duration = Duration::from_millis(250);
}

/// CAN bus configuration.
pub mod can {
    use core::time::Duration;

    /// Max number of `send()` operations in the same loop iteration.
    pub const MAX_CONCURRENT_SEND: usize = 30;
    /// Max number of `receive()` operations in the same loop iteration.
    pub const MAX_CONCURRENT_RECEIVE: usize = 30;

    /// Time window used to estimate the CAN bus load.
    pub const LOAD_ESTIMATION_WINDOW: Duration = Duration::from_secs(1);
    /// Multiplicative factor (between 0 and 100) used for the Moving Weighted Average (MWA) to compute the current CAN
    /// bus load: `load = MWA/100 * previous + (1 - MWA/100) * current`.
    pub const LOAD_ESTIMATION_MWA_WEIGHT: usize = 75;

    /// Rate at which to send board status messages.
    pub const STATUS_REPORT_INTERVAL: Duration = Duration::from_secs(1);
}

/// Tacom configuration.
pub mod tacom {
    use core::time::Duration;

    /// Rate at which to send Tacom ping messages (if enabled).
    pub const PING_INTERVAL: Duration = Duration::from_secs(3);
}

//! Hardware Abstraction Layer.

pub(crate) mod ota;
pub mod task;

/// Initializes the ESP hardware and platform.
/// Note: it is advisable to keep the watchdog enabled, but the option is there to avoid issues in the examples.
///
/// # Errors
///
/// This function returns an error if it fails to initialize the ESP logger or if it cannot confiure the task watchdog.
pub fn init(disable_watchdog: bool) -> Result<(), Error> {
    // It is necessary to call this function once, otherwise some patches to the runtime implemented by `esp-idf-sys`
    // might not link properly
    // Reference: <https://github.com/esp-rs/esp-idf-template/issues/71>
    let _ = esp_idf_sys::link_patches();

    // Initialize logger
    esp_idf_svc::log::EspLogger::initialize_default();
    #[cfg(all(debug_assertions, feature = "log-trace"))]
    esp_idf_svc::log::set_target_level("*", log::LevelFilter::Trace).map_err(|_| Error::Logger)?;
    #[cfg(all(debug_assertions, not(feature = "log-trace")))]
    esp_idf_svc::log::set_target_level("*", log::LevelFilter::Debug).map_err(|_| Error::Logger)?;
    #[cfg(not(debug_assertions))]
    esp_idf_svc::log::set_target_level("*", log::LevelFilter::Info).map_err(|_| Error::Logger)?;
    log::trace!("Successfully initialized logger");

    // Filter unwanted logs that are to verbose
    esp_idf_svc::log::set_target_level("pthread", log::LevelFilter::Info)
        .map_err(|_| Error::Logger)?;
    esp_idf_svc::log::set_target_level("RANDOM", log::LevelFilter::Info)
        .map_err(|_| Error::Logger)?;
    esp_idf_svc::log::set_target_level("esp_idf_svc::timer", log::LevelFilter::Info)
        .map_err(|_| Error::Logger)?;
    esp_idf_svc::log::set_target_level("rmt(legacy)", log::LevelFilter::Info)
        .map_err(|_| Error::Logger)?;

    // Disable watchdog if requested
    if disable_watchdog {
        let wtd_config = &esp_idf_sys::esp_task_wdt_config_t {
            timeout_ms: 10000,
            trigger_panic: false,
            ..Default::default()
        };
        #[allow(unsafe_code)]
        esp_idf_sys::EspError::check_and_return(
            unsafe { esp_idf_sys::esp_task_wdt_reconfigure(wtd_config) },
            (),
        )
        .map_err(|_| Error::Watchdog)?;
    }

    Ok(())
}

/// Errors related to the ESP platform and hardware.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failure during ESP logger initialization.
    #[error("logger initialization failure")]
    Logger,
    /// Failure during task watchdog configuration.
    #[error("task watchdog configuration failure")]
    Watchdog,
}

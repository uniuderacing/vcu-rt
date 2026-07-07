//! Abstractions to handle Over-The-Air updates using Tacom.

use esp_idf_svc::ota::{EspOta, EspOtaUpdate};
use std::sync::OnceLock;
use tacom::ota::{Mcu, OtaError};

/// Hack to avoid self referential structs.
static mut ESP_OTA: OnceLock<EspOta> = OnceLock::new();

/// Update manager for Vehicle Control Unit.
#[derive(Debug)]
pub struct VcuUpdateManager<'a> {
    /// Unique ID of the VCU.
    vcu_id: u8,
    /// Handler for OTA updates.
    ota_handler: Option<EspOtaUpdate<'a>>,
}

impl Mcu<u8> for VcuUpdateManager<'_> {
    fn begin(board: u8) -> Result<Self, OtaError> {
        log::info!("Starting OTA update for VCU with ID {board:#04x}");
        #[allow(unsafe_code)]
        #[allow(static_mut_refs)]
        // Safety: begin is only called once in the program
        let esp_ota = unsafe { ESP_OTA.get_mut_or_init(|| EspOta::new().unwrap()) };
        let ota_handler = esp_ota.initiate_update().unwrap();
        Ok(Self {
            vcu_id: board,
            ota_handler: Some(ota_handler),
        })
    }

    fn whoami(&self) -> &u8 {
        &self.vcu_id
    }

    fn write_chunk_to_partition(&mut self, data: &[u8]) -> Result<(), OtaError> {
        let ota_handler = self.ota_handler.as_mut().ok_or(OtaError::Generic)?;
        ota_handler.write(data).map_err(|_| OtaError::Generic)
    }

    fn finalize(&mut self) -> Result<(), OtaError> {
        log::info!("Finalizing OTA update for VCU with ID {:#04x}", self.vcu_id);
        let ota_handler = self.ota_handler.take().ok_or(OtaError::Generic)?;
        let completed_update = ota_handler.finish().map_err(|_| OtaError::Generic)?;

        // Set the new partition as boot partition
        completed_update.activate().map_err(|_| OtaError::Generic)
    }

    fn reboot() -> ! {
        #[allow(unsafe_code)]
        unsafe {
            esp_idf_sys::esp_restart()
        }
    }

    fn validate() {
        #[allow(unsafe_code)]
        let _ = unsafe { esp_idf_sys::esp_ota_mark_app_valid_cancel_rollback() };
    }
}

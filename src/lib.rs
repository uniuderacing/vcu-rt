//! An asynchronous runtime allowing to easily program the behavior of on-board Vehicle Control Units.

#![feature(once_cell_get_mut)]
//#![feature(const_vec_string_slice)] // Stabilized in 1.87, not yet available in the ESP toolchain so clippy complains

mod config;
pub mod hal;
pub mod param;
pub mod prelude;
pub mod task;

use core::fmt::{self, Debug};
use esp_idf_hal::can;
use param::{
    AsyncAdc, DiscoveryCallback, Executor, ExecutorError, SharedBoardManager, TacomParameter,
    VcuResources,
};
use serena_messages::{ResetReasonMessage, ResetReasonMessageResetReason as ResetReason};
use std::{
    sync::{Arc, Mutex as SyncMutex},
    time::Instant,
};
use tacom::{
    master::{board::Board, parameter::Parameter},
    slave::parameter::UpdateableParameter,
};
use task::ITask;

// +---------+
// | Runtime |
// +---------+

/// Runtime associated to a Vehicle Control Unit.
pub struct Runtime {
    /// Unique ID of the VCU.
    vcu_id: u8,
    /// Human-readable name of the VCU.
    vcu_name: String,
    /// Asynchronous executor for tasks run by this VCU.
    executor: Executor,
    /// List of task definitions managed by the runtime.
    task_definitions: Vec<Box<dyn ITask>>,
    /// List of running tasks managed by the runtime.
    /// When these are dropped, the corresponding tasks get dropped by the executor (without needing extra signalling).
    task_handles: Vec<futures::future::RemoteHandle<()>>,
    /// Optional Tacom board manager (if master functionalities are enabled).
    tacom_board_manager: Option<SharedBoardManager<'static>>,
}

impl Runtime {
    /// Entrypoint of the builder pattern.
    /// Returns a default instance of [`Builder`].
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Constructs an instance of [`Runtime`] using the given [`Builder`] instance.
    ///
    /// # Errors
    ///
    /// This method may return an error if it fails to construct the internal asynchronous task executor.
    #[allow(clippy::too_many_lines)]
    fn new(builder: Builder) -> Result<Self, Error> {
        log::debug!(
            "Building VCU runtime for \"{}\" (ID {:#04x})",
            builder.vcu_name,
            builder.vcu_id
        );

        // Collect the reason of the last VCU reset
        #[allow(unsafe_code)]
        let reset_reason = match unsafe { esp_idf_sys::esp_reset_reason() } {
            esp_idf_sys::esp_reset_reason_t_ESP_RST_BROWNOUT => ResetReason::Esp_rst_brownout,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_DEEPSLEEP => ResetReason::Esp_rst_deepsleep,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_EXT => ResetReason::Esp_rst_ext,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_INT_WDT => ResetReason::Esp_rst_int_wdt,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_PANIC => ResetReason::Esp_rst_panic,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_POWERON => ResetReason::Esp_rst_poweron,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_SDIO => ResetReason::Esp_rst_sdio,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_SW => ResetReason::Esp_rst_sw,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_TASK_WDT => ResetReason::Esp_rst_task_wdt,
            esp_idf_sys::esp_reset_reason_t_ESP_RST_WDT => ResetReason::Esp_rst_wdt,
            _ => ResetReason::Esp_rst_unknown,
        };
        log::info!("Reset reason: {reset_reason:?}");
        let reset_reason_message = ResetReasonMessage::new()
            .with_board_id(builder.vcu_id)
            .with_reset_reason(reset_reason);

        // Determine if there are any critical tasks
        let is_can_critical = builder
            .can_driver
            .as_ref()
            .is_some_and(|(_, _, critical)| *critical);
        let are_there_critical_tasks = is_can_critical
            || builder
                .task_definitions
                .iter()
                .any(|task| task.is_critical());

        // Construct asynchronous executor
        let executor = Executor::new(are_there_critical_tasks).map_err(Error::Executor)?;

        // Add internal CAN task
        let mut task_definitions: Vec<Box<dyn ITask>> = vec![];
        let vcu_id = builder.vcu_id;
        let tacom_parameters = builder.tacom_parameters;
        let tacom_ota_enable = builder.tacom_ota_enable;
        let tacom_master_enable = &builder.tacom_master_enable;
        let can_management = builder.can_driver.map_or_else(
            || {
                log::warn!("This VCU runtime will not manage CAN communication");
                None
            },
            |(driver, can_queue_lengths, critical)| {
                let (task, message_dispatcher, board_manager, can_resources) =
                    task::internal::can_manager(
                        vcu_id,
                        driver,
                        can_queue_lengths,
                        critical,
                        tacom_parameters,
                        tacom_ota_enable,
                        tacom_master_enable,
                    );
                task_definitions.push(Box::new(task));
                Some((message_dispatcher, board_manager, can_resources))
            },
        );

        // Add internal status task
        if can_management.is_some() && builder.board_status_enable {
            let task = task::internal::status(builder.vcu_id);
            task_definitions.push(Box::new(task));
        }

        // Add internal Tacom ping task
        if can_management.is_some() && builder.tacom_ping_enable {
            let task = task::internal::tacom_ping(builder.vcu_id);
            task_definitions.push(Box::new(task));
        }

        // Add internal power monitor task
        let power_consumption = builder.efuse_adc.map_or_else(
            || None,
            |efuse_adc| {
                let (task, power_consumption) =
                    task::internal::power_monitor(efuse_adc, builder.efuse_constants);
                task_definitions.push(Box::new(task));
                Some(power_consumption)
            },
        );

        // Add user tasks
        task_definitions.extend(builder.task_definitions);

        // Initialize all tasks
        let (message_dispatcher, board_manager, can_resources) = match can_management {
            Some((a, b, c)) => (Some(a), Some(b), Some(c)),
            None => (None, None, None),
        };
        let tacom_board_manager = board_manager.flatten();
        let vcu_resources = VcuResources {
            executor: executor.clone(),
            power_consumption,
            can_bus: can_resources,
            tacom_board_manager: tacom_board_manager.clone(),
        };
        for task in &mut task_definitions {
            let message_dispatcher_2 = message_dispatcher.clone();
            let vcu_resources_2 = vcu_resources.clone();
            futures::executor::block_on(async {
                task.init(message_dispatcher_2, vcu_resources_2).await;
            });
        }

        // Push the reset reason message to the CAN queue
        if let Some(can_resources) = &vcu_resources.can_bus {
            can_resources.stream.send(reset_reason_message);
        }

        // Construct and return runtime
        Ok(Self {
            vcu_id: builder.vcu_id,
            vcu_name: builder.vcu_name,
            executor,
            task_definitions,
            task_handles: vec![],
            tacom_board_manager,
        })
    }

    /// Returns the unique ID of the VCU associated with this [`Runtime`].
    #[must_use]
    pub const fn vcu_id(&self) -> u8 {
        self.vcu_id
    }

    /// Returns the name of the VCU associated with this [`Runtime`].
    #[must_use]
    pub const fn vcu_name(&self) -> &str {
        self.vcu_name.as_str()
    }

    /// Starts the [`Runtime`] and the tasks it manages.
    /// Notice that this call is indefinitely blocking, i.e. it never returns.
    ///
    /// # Panics
    ///
    /// This method panics if some task fails to be spawned.
    pub fn start(&mut self) {
        for task in self.task_definitions.drain(..) {
            let task_name = task.name().to_owned();
            let task_is_critical = task.is_critical();
            log::debug!(
                "Spawning {}task \"{}\"",
                if task_is_critical { "critical " } else { "" },
                task_name
            );

            let handle = self
                .executor
                .spawn(
                    async move {
                        log::debug!("Task \"{task_name}\" started");
                        task.run().await;
                        log::debug!("Task \"{task_name}\" terminated");
                    },
                    task_is_critical,
                )
                .unwrap();
            self.task_handles.push(handle);
        }

        // Block indefinitely until cancellation
        futures::executor::block_on(core::future::pending::<()>());
    }
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tacom_board_manager = if self.tacom_board_manager.is_some() {
            "Some( ... )"
        } else {
            "None"
        };

        f.debug_struct("Runtime")
            .field("vcu_id", &self.vcu_id)
            .field("vcu_name", &self.vcu_name)
            .field("executor", &self.executor)
            .field("task_definitions", &self.task_definitions)
            .field("task_handles", &self.task_handles)
            .field("tacom_board_manager", &tacom_board_manager)
            .finish()
    }
}

// +-----------------+
// | Runtime builder |
// +-----------------+

/// Builder type for [`Runtime`].
#[allow(clippy::struct_excessive_bools)]
#[must_use]
pub struct Builder {
    /// Unique ID of the VCU.
    vcu_id: u8,
    /// Human-readable name of the VCU.
    vcu_name: String,
    /// List of Tacom parameters attached to the VCU.
    tacom_parameters: Vec<TacomParameter>,
    /// List of user task definitions managed by the [`Runtime`].
    task_definitions: Vec<Box<dyn ITask>>,
    /// Optional CAN bus driver.
    /// The second argument indicates the optional queue sizes for high- and low-priority messages, respectively.
    /// The additional boolean value is used to indicate whether the CAN manager task is critical/high-priority or not.
    #[allow(clippy::type_complexity)]
    can_driver: Option<(can::CanDriver<'static>, Option<(usize, usize)>, bool)>,
    /// Optional hardware interface with the VCU eFuse to read its power consumption feedback signal.
    efuse_adc: Option<Box<dyn AsyncAdc + Send>>,
    /// Values of the two constants `G_imon` and `R_ilm` for the eFuse.
    efuse_constants: (f64, f64),
    /// Whether to enable board status messages.
    board_status_enable: bool,
    /// Whether to enable Tacom ping messages.
    tacom_ping_enable: bool,
    /// Whether to enable Tacom OTAs.
    tacom_ota_enable: bool,
    /// Whether to enable Tacom master functionalities.
    /// The second argument indicates the optional discovery callback.
    tacom_master_enable: (bool, Option<DiscoveryCallback<'static>>),
}

impl Builder {
    /// Adds a user-defined ID to the VCU.
    pub const fn with_vcu_id(mut self, vcu_id: u8) -> Self {
        self.vcu_id = vcu_id;
        self
    }

    /// Adds a user-defined name to the VCU.
    pub fn with_vcu_name(mut self, vcu_name: &str) -> Self {
        vcu_name.clone_into(&mut self.vcu_name);
        self
    }

    /// Adds a Tacom parameter for the VCU running the [`Runtime`].
    pub fn with_parameter<S>(mut self, parameter: S) -> Self
    where
        S: UpdateableParameter + Send + Sync + 'static,
    {
        self.tacom_parameters.push(TacomParameter::new(parameter));
        self
    }

    /// Adds a task to be managed by the [`Runtime`].
    pub fn with_task<T>(mut self, task: T) -> Self
    where
        T: ITask + 'static,
    {
        self.task_definitions.push(Box::new(task));
        self
    }

    /// Adds a CAN bus driver.
    /// If provided, the [`Runtime`] will entirely manage CAN bus communication.
    /// In such case, all tasks will be provided with a channel sender to send CAN messages.
    /// The parameter `queue_lengths` can be used to optionally provide custom sizes for the internal high- and
    /// low-priority queues for CAN bus messages, respectively. If left to `None`, the default sizes `(48, 64)` will be
    /// used.
    /// The `critical` flag indicated whether the CAN bus manager task is critical or not.
    pub fn with_can_driver(
        mut self,
        driver: can::CanDriver<'static>,
        queue_lengths: Option<(usize, usize)>,
        critical: bool,
    ) -> Self {
        self.can_driver = Some((driver, queue_lengths, critical));
        self
    }

    /// Adds a hardware interface with the VCU's eFuse.
    /// If provided, the [`Runtime`] will periodically read the voltage level on the provided pin.
    /// The computed power consumption is made available to all user tasks via [`VcuResources`].
    pub fn with_efuse<A>(
        mut self,
        efuse_adc: A,
        custom_g_imon: Option<f64>,
        custom_r_ilm: Option<f64>,
    ) -> Self
    where
        A: AsyncAdc + Send + 'static,
    {
        let g_imon = custom_g_imon.unwrap_or(config::hw::EFUSE_DEFAULT_G_IMON);
        let r_ilm = custom_r_ilm.unwrap_or(config::hw::EFUSE_DEFAULT_R_ILM);
        self.efuse_adc = Some(Box::new(efuse_adc));
        self.efuse_constants = (g_imon, r_ilm);
        self
    }

    /// Defines whether to enable board status messages or not.
    /// Note that this can be enabed only if both a CAN bus driver and an eFuse ADC are provided.
    pub const fn with_board_status(mut self, enable: bool) -> Self {
        self.board_status_enable = enable;
        self
    }

    /// Defines whether to enable Tacom ping messages or not.
    /// Note that Tacom features can be enabed only if a CAN bus driver is provided.
    pub const fn with_tacom_ping(mut self, enable: bool) -> Self {
        self.tacom_ping_enable = enable;
        self
    }

    /// Defines whether to enable Tacom OTA updates or not.
    /// Note that Tacom features can be enabed only if a CAN bus driver is provided.
    pub const fn with_tacom_ota(mut self, enable: bool) -> Self {
        self.tacom_ota_enable = enable;
        self
    }

    /// Defines whether to enable Tacom master functionalities or not.
    /// Note that Tacom features can be enabed only if a CAN bus driver is provided.
    /// Warning: the discovery callback MUST NOT lock any of the mutexes in its arguments, as this would certainly cause
    /// a deadlock.
    pub fn with_tacom_master<CB>(mut self, enable: bool, discovery_callback: Option<CB>) -> Self
    where
        CB: Fn(Arc<SyncMutex<Board>>, Vec<Arc<SyncMutex<Parameter>>>, Instant)
            + Send
            + Sync
            + 'static,
    {
        let discovery_callback = discovery_callback.map(|cb| {
            let coerced: DiscoveryCallback<'_> = Arc::new(cb);
            coerced
        });
        self.tacom_master_enable = (enable, discovery_callback);
        self
    }

    /// Builds an instance of [`Runtime`] using the internal state of `self`.
    ///
    /// # Errors
    ///
    /// This method returns an error if and only if `Runtime::new()` returns an error.
    pub fn build(self) -> Result<Runtime, Error> {
        Runtime::new(self)
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            vcu_id: u8::MAX, // Unused ID, will most probably result in an error if not set
            vcu_name: "Unnamed VCU".to_owned(),
            tacom_parameters: vec![],
            task_definitions: vec![],
            can_driver: None,
            efuse_adc: None,
            efuse_constants: (
                config::hw::EFUSE_DEFAULT_G_IMON,
                config::hw::EFUSE_DEFAULT_R_ILM,
            ),
            board_status_enable: true,
            tacom_ping_enable: true,
            tacom_ota_enable: true,
            tacom_master_enable: (false, None), // Only Tacom slave by default
        }
    }
}

impl Debug for Builder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tacom_parameters = self
            .tacom_parameters
            .iter()
            .map(|parameter| {
                format!(
                    "UpdateableParameter {{ name: \"{}\", .. }}",
                    parameter.name()
                )
            })
            .collect::<Vec<_>>();
        let can_driver = if self.can_driver.is_some() {
            "Some( ... )"
        } else {
            "None"
        };
        let efuse_adc = if self.efuse_adc.is_some() {
            "Some( ... )"
        } else {
            "None"
        };
        let discovery_cb = if self.tacom_master_enable.1.is_some() {
            "Some( ... )"
        } else {
            "None"
        };

        f.debug_struct("RuntimeBuilder")
            .field("vcu_id", &self.vcu_id)
            .field("vcu_name", &self.vcu_name)
            .field("tacom_parameters", &tacom_parameters)
            .field("task_definitions", &self.task_definitions)
            .field("can_driver", &can_driver)
            .field("efuse_adc", &efuse_adc)
            .field("efuse_constants", &self.efuse_constants)
            .field("board_status_enable", &self.board_status_enable)
            .field("tacom_ping_enable", &self.tacom_ping_enable)
            .field("tacom_ota_enable", &self.tacom_ota_enable)
            .field(
                "tacom_master_enable",
                &(self.tacom_master_enable.0, discovery_cb),
            )
            .finish()
    }
}

// +----------------+
// | Runtime errors |
// +----------------+

/// Errors generated by a [`Runtime`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failure related to the [`Runtime`] async task executor.
    #[error("async task executor failure")]
    Executor(ExecutorError),
}

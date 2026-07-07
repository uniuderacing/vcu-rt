//! Task that manages the CAN bus.

use crate::{
    config, hal,
    param::{
        CanBusLoad, CanResources, CanStream, DiscoveryCallback, SharedBoardManager,
        SharedMessageDispatcher, TacomParameter, VcuResources,
    },
    task::{IAction, ITask, Recurrent},
};
use anyhow::Result as AnyResult;
use async_trait::async_trait;
use core::{
    fmt::{self, Debug},
    ops::Deref,
    time::Duration,
};
use embedded_can::{blocking::Can as ICan, nb::Can as INbCan};
use esp_idf_hal::can;
use esp_idf_sys::EspError;
use futures::lock::Mutex as AsyncMutex;
use messages::Frame;
use portable_atomic::AtomicUsize;
use std::{
    collections::VecDeque,
    sync::{atomic::Ordering, Arc, Mutex as SyncMutex},
    time::Instant,
};
use tacom::{
    interface::{MessagingInterface, SendingInterface},
    master::BoardManager,
    ota::OtaReceiverDriver,
    slave::MessageDispatcher,
};

/// Returns a new instance of the CAN manager task, together with:
/// - a reference to a Tacom [`MessageDispatcher`](tacom::slave::MessageDispatcher), which can be used to initialize
///   tasks that need to register callbacks tied to specific CAN message types;
/// - a set of resources which can be used by other tasks to interact with the CAN bus.
pub fn can_manager(
    vcu_id: u8,
    driver: can::CanDriver<'static>,
    can_queue_lengths: Option<(usize, usize)>,
    critical: bool,
    tacom_parameters: Vec<TacomParameter>,
    tacom_ota_enable: bool,
    tacom_master_enable: &(bool, Option<DiscoveryCallback<'static>>),
) -> (
    impl ITask,
    SharedMessageDispatcher<'static>,
    Option<SharedBoardManager<'static>>,
    CanResources,
) {
    // Create the message dispatcher
    let shared_driver = Arc::new(MutexCanDriver::new(driver));
    let mut message_dispatcher =
        MessageDispatcher::<'_, _, hal::ota::VcuUpdateManager>::new(shared_driver.clone());

    // Attach Tacom parameters
    for parameter in tacom_parameters {
        message_dispatcher = message_dispatcher.attach_parameter(parameter);
    }

    // Enable Tacom OTA if requested
    let message_dispatcher = if tacom_ota_enable {
        let ota_receiver_driver = OtaReceiverDriver::init(vcu_id);
        message_dispatcher.attach_ota(ota_receiver_driver)
    } else {
        message_dispatcher
    };

    // Create Tacom board manager if requested
    let discovery_callback = tacom_master_enable.1.clone();
    let shared_board_manager = if tacom_master_enable.0 {
        let mut board_manager = BoardManager::default();
        board_manager.on_discovery(move |boards| {
            for (board, instant) in boards {
                let mut board_lock = board.lock().unwrap();
                log::debug!("Discovered new board with ID {}", board_lock.id());
                board_lock.request_list_parameters().unwrap();
                let board_2 = board.clone();
                let discovery_callback_2 = discovery_callback.clone();
                board_lock.on_discovery(move |parameters| {
                    for parameter in &parameters {
                        log::debug!(
                            "Discovered new parameter with ID {}",
                            parameter.lock().unwrap().id()
                        );
                    }
                    // Call user-defined callback
                    if let Some(cb) = discovery_callback_2.clone() {
                        log::trace!("Calling user-defined discovery callback");
                        cb(board_2.clone(), parameters, instant);
                    }
                });
            }
        });
        Some(Arc::new(AsyncMutex::new(board_manager)))
    } else {
        None
    };

    // Create the CAN manager task
    let shared_message_dispatcher = Arc::new(AsyncMutex::new(Some(message_dispatcher)));
    let can_stream = CanStream::new(can_queue_lengths);
    let action = CanManager::new(
        shared_driver,
        shared_message_dispatcher.clone(),
        shared_board_manager.clone(),
        &can_stream,
    );
    let can_load = action.load_manager.load();

    let can_resources = CanResources {
        stream: can_stream,
        load: can_load,
    };
    (
        Recurrent::new("CAN manager", critical, None, action),
        shared_message_dispatcher,
        shared_board_manager,
        can_resources,
    )
}

// +--------+
// | Action |
// +--------+

/// Representation of the action that manages the CAN bus interface.
pub struct CanManager {
    /// CAN bus driver.
    driver: Arc<MutexCanDriver<'static>>,
    /// Tacom's message dispatcher.
    /// Invariant: if the mutex is unlocked, then the enclosed option must be `Some(_)`.
    message_dispatcher: SharedMessageDispatcher<'static>,
    /// Tacom's board manager.
    board_manager: Option<SharedBoardManager<'static>>,
    /// Shared channel that other tasks may use to send CAN messages.
    send_queue: CanStream,
    /// Set of resources that can be used to interact with the VCU hardware.
    vcu_resources: Option<VcuResources>,
    /// CAN bus load manager.
    load_manager: LoadManager,
}

impl CanManager {
    /// Constructs a new instance of [`CanManager`].
    fn new(
        driver: Arc<MutexCanDriver<'static>>,
        message_dispatcher: SharedMessageDispatcher<'static>,
        board_manager: Option<SharedBoardManager<'static>>,
        can_stream: &CanStream,
    ) -> Self {
        Self {
            driver,
            message_dispatcher,
            board_manager,
            send_queue: can_stream.clone(),
            vcu_resources: None,
            load_manager: LoadManager::new(),
        }
    }
}

#[async_trait]
impl IAction for CanManager {
    type Input = ();

    async fn init(&mut self, vcu_resources: VcuResources) {
        // This method is called only after the task is created, which is done only if a `CanDriver` was provided,
        // therefore the resources must exist
        std::assert!(vcu_resources.can_bus.is_some());

        // Even if they are not used, storing this here means that the `send_queue` cannot be mistakenly dropped
        self.vcu_resources = Some(vcu_resources);

        // Start the hardware driver
        self.driver
            .lock()
            .unwrap()
            .start()
            .unwrap_or_else(|_| log::error!("Failed to start CAN driver"));
    }

    async fn run(&mut self, _input: Self::Input) {
        // Send multiple frames in the queue, if present
        let mut tries = 0;
        while let Some(message) = self.send_queue.try_recv() {
            let frame = Frame::from(message);
            try_can_send(&mut self.driver.lock().unwrap(), &frame)
                .inspect(|()| log::trace!("Sent CAN frame: {frame:?}"))
                .expect("Irrecoverable CAN bus driver failure");
            self.load_manager.ingest(&frame);
            tries += 1;
            if tries >= config::can::MAX_CONCURRENT_SEND {
                break;
            }
        }

        // Receive multiple frames, if available
        let mut queue = VecDeque::with_capacity(config::can::MAX_CONCURRENT_RECEIVE);
        tries = 0;
        #[allow(clippy::significant_drop_in_scrutinee)]
        while let Some(sent) = try_can_receive(&mut self.driver.lock().unwrap())
            .expect("Irrecoverable CAN bus driver failure")
        {
            queue.push_back(sent);
            tries += 1;
            if tries >= config::can::MAX_CONCURRENT_RECEIVE {
                break;
            }
        }
        for frame in queue {
            log::trace!("Received CAN frame: {frame:?}");
            self.load_manager.ingest(&frame);
            // Dispatch received frame and wake appropriate tasks
            if let Some(board_manager) = self.board_manager.as_ref() {
                board_manager
                    .lock()
                    .await
                    .ingest(&frame, self.driver.clone());
            }
            let _ = self
                .message_dispatcher
                .lock()
                .await
                .as_mut()
                .unwrap() // Safe due to `message_dispatcher` invariant
                .ingest(&frame);
        }

        // Let the load manager perform its calculations
        self.load_manager.update();
    }
}

// +--------------+
// | Load manager |
// +--------------+

/// CAN bus load manager.
struct LoadManager {
    /// Total number of bits sent/received on the CAN bus since `start`.
    bit_count: usize,
    /// Point in time when the count was last cleared.
    start: Instant,
    /// Shared reference to the current CAN bus load.
    bit_load: Arc<AtomicUsize>,
}

impl LoadManager {
    /// Constructs a new instance of [`LoadManager`].
    fn new() -> Self {
        Self {
            bit_count: 0,
            start: Instant::now(),
            bit_load: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns an (owned) reference to the estimated bit load of the CAN bus.
    fn load(&self) -> CanBusLoad {
        CanBusLoad::new(self.bit_load.clone())
    }

    /// Updates the internal state, recomputing the load estimate if necessary.
    fn update(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > config::can::LOAD_ESTIMATION_WINDOW {
            // Compute the current load
            let previous_bit_load = self.bit_load.load(Ordering::SeqCst);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let current_bit_load = ((self.bit_count as f64)
                * elapsed.div_duration_f64(config::can::LOAD_ESTIMATION_WINDOW))
            .round() as usize;
            let new_bit_load = (config::can::LOAD_ESTIMATION_MWA_WEIGHT * previous_bit_load
                + (100 - config::can::LOAD_ESTIMATION_MWA_WEIGHT) * current_bit_load)
                / 100;
            self.bit_load.store(new_bit_load, Ordering::SeqCst);

            // Reset the window
            self.bit_count = 0;
            self.start = Instant::now();
        }
    }

    /// Processes a frame that was sent/received on the CAN bus.
    fn ingest(&mut self, frame: &Frame) {
        // Process the given frame
        let data_length = 8 * frame.get_data().len(); // In bits
        let overhead = if frame.is_extended() {
            // Header: SOF (1 bit) + ID (11 bit) + SSR (1 bit) + IDE (1 bit)
            // Control field: ID (18 bit) + RTR (1 bit) + other (6 bit)
            // Trailer: CRC (15 bit) + delimiter (3 bit) + EOF (7 bit)
            // Inter-frame pause (3 bit)
            67
        } else {
            // Header: SOF (1 bit) + ID (11 bit) + RTR (1 bit)
            // Control field (6 bit)
            // Trailer: CRC (15 bit) + delimiter (3 bit) + EOF (7 bit)
            // Inter-frame pause (3 bit)
            47
        };
        let total = data_length + overhead;
        self.bit_count += total + (total / (5 * 16)); // Adds bit stuffing average (2/(2^5) of max bit stuffing)

        self.update();
    }
}

// +---------------------+
// | Messaging interface |
// +---------------------+

/// Wrapped instance of [`CanDriver`](can::CanDriver) with synchronized access using a mutex.
struct MutexCanDriver<'a>(SyncMutex<can::CanDriver<'a>>);

impl<'a> MutexCanDriver<'a> {
    /// Constructs a new instance of [`MutexCanDriver`] by wrapping the given [`CanDriver`](can::CanDriver).
    const fn new(driver: can::CanDriver<'a>) -> Self {
        Self(SyncMutex::new(driver))
    }
}

impl SendingInterface for MutexCanDriver<'_> {
    fn send(&self, frame: Frame) -> AnyResult<()> {
        if let Ok(ref mut guard) = self.try_lock() {
            try_can_send(guard, &frame)?;
        }
        Ok(())
    }

    fn send_blocking(&self, frame: Frame) -> AnyResult<()> {
        can_send(&mut self.lock().unwrap(), &frame).map_err(Into::into)
    }
}

impl MessagingInterface for MutexCanDriver<'_> {
    fn receive(&self) -> AnyResult<Option<Frame>> {
        let frame = if let Ok(ref mut guard) = self.try_lock() {
            try_can_receive(guard)?
        } else {
            None
        };

        Ok(frame)
    }

    fn receive_blocking(&self) -> AnyResult<Frame> {
        can_receive(&mut self.lock().unwrap()).map_err(Into::into)
    }
}

impl Debug for MutexCanDriver<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutexCanDriver")
            .field(
                "lock_status",
                if self.try_lock().is_ok() {
                    &"unlocked"
                } else {
                    &"locked"
                },
            )
            .finish_non_exhaustive()
    }
}

impl<'a> Deref for MutexCanDriver<'a> {
    type Target = SyncMutex<can::CanDriver<'a>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// +-------+
// | Utils |
// +-------+

/// Tries to send a single frame on the CAN bus.
/// If the CAN bus driver transmit queue is full, this function does not block but it may drop the current frame or a
/// low-priority one.
///
/// # Errors
///
/// This function only returns an error if the CAN bus driver is in a faulty state and cannot be recovered.
fn try_can_send(driver: &mut can::CanDriver<'_>, frame: &Frame) -> Result<(), EspError> {
    let can_frame = frame.as_can_frame();
    match INbCan::transmit(driver, &can_frame) {
        Ok(None) => Ok(()),
        Ok(Some(dropped_can_frame)) => {
            log::warn!(
                "CAN bus transmit queue is full, dropping low-priority frame {dropped_can_frame}",
            );
            Ok(())
        }
        Err(nb::Error::WouldBlock) => {
            log::warn!("CAN bus transmit queue is full, dropping current frame {can_frame}");
            Ok(())
        }
        Err(nb::Error::Other(err)) if err.cause().code() == esp_idf_sys::ESP_ERR_INVALID_STATE => {
            log::warn!("CAN bus error on send: {err:?}");
            log::warn!("CAN bus driver is not in running state, or not installed");
            log::warn!("Current frame will be dropped: {can_frame}");
            try_can_recover(driver)
        }
        Err(nb::Error::Other(err)) => {
            log::error!("CAN bus error on send: {err:?}");
            Err(err.cause())
        }
    }
}

/// Sends a single frame on the CAN bus.
/// If the transmit queue is full, this function blocks until some new space frees up.
///
/// # Errors
///
/// This function only returns an error if the CAN bus driver is in a faulty state.
/// It does not tries to recover it (as [`try_can_send()`] does), since it may result in frame losses.
fn can_send(driver: &mut can::CanDriver<'_>, frame: &Frame) -> Result<(), EspError> {
    let can_frame = frame.as_can_frame();
    ICan::transmit(driver, &can_frame).map_err(|err| {
        log::error!("CAN bus error on send: {err:?}");
        err.cause()
    })
}

/// Tries to receive a single frame from the CAN bus.
/// Since it does not block, it returns `Ok(None)` if there is not frame to be received.
///
/// # Errors
///
/// This function only returns an error if the CAN bus driver is in a faulty state and cannot be recovered.
fn try_can_receive(driver: &mut can::CanDriver<'_>) -> Result<Option<Frame>, EspError> {
    match INbCan::receive(driver) {
        Ok(can_frame) => Ok(Some(Frame::new_from_can_frame(&can_frame, None).expect(
            "Failed to convert raw CAN frame into message frame (should never happen)",
        ))),
        Err(nb::Error::WouldBlock) => Ok(None),
        Err(nb::Error::Other(err)) if err.cause().code() == esp_idf_sys::ESP_ERR_INVALID_STATE => {
            log::warn!("CAN bus error on receive: {err:?}");
            log::warn!("CAN bus driver is not in running state, or not installed");
            try_can_recover(driver).map(|()| None)
        }
        Err(nb::Error::Other(err)) => {
            log::error!("CAN bus error on receive: {err:?}");
            Err(err.cause())
        }
    }
}

/// Receives a single frame from the CAN bus.
/// This function blocks until a frame is available to be received.
///
/// # Errors
///
/// This function only returns an error if the CAN bus driver is in a faulty state.
/// It does not tries to recover it (as [`try_can_receive()`] does), since it may result in this function call returning
/// before receiving a frame.
fn can_receive(driver: &mut can::CanDriver<'_>) -> Result<Frame, EspError> {
    ICan::receive(driver)
        .map(|can_frame| {
            Frame::new_from_can_frame(&can_frame, None)
                .expect("Failed to convert raw CAN frame into message frame (should never happen)")
        })
        .map_err(|err| {
            log::error!("CAN bus error on receive: {err:?}");
            err.cause()
        })
}

/// Tries to recover a faulty CAN bus driver state.
///
/// # Errors
///
/// This function returns an error if the CAN bus driver cannot be recovered.
#[allow(clippy::needless_pass_by_ref_mut)]
fn try_can_recover(_driver: &mut can::CanDriver<'_>) -> Result<(), EspError> {
    log::info!("Trying to recover CAN bus driver state");

    #[allow(unsafe_code)]
    unsafe {
        // Reconfigure driver
        esp_idf_sys::esp!(esp_idf_sys::twai_reconfigure_alerts(
            esp_idf_sys::TWAI_ALERT_BUS_RECOVERED,
            std::ptr::null_mut(),
        ))?;
        esp_idf_sys::esp!(esp_idf_sys::twai_initiate_recovery())?;

        // Try to restart
        while esp_idf_sys::esp!(esp_idf_sys::twai_start()).is_err() {
            // Not a great solution because it may "monopolize" one of the asynchronous executor's worker threads, but
            // at least it prevents it from getting it stuck in this loop, thus triggering the task watchdog (TWDT).
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    log::info!("Successfully recovered CAN bus driver state");
    Ok(())
}

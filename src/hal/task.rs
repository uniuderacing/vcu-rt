//! Primitives to handle asynchronous tasks running on the ESP.

use core::time::Duration;
use esp_idf_svc::timer::{EspAsyncTimer, EspTaskTimerService, EspTimer};
use esp_idf_sys::EspError;
use futures::channel::oneshot;
use std::sync::OnceLock;

/// Static (lazily-initialized) instance of an [`EspTaskTimerService`], which can be used to access The ESP timer
/// service. The service is initialized the first time it is used.
static HW_TIMER_SERVICE: OnceLock<EspTaskTimerService> = OnceLock::new();

/// Yields from the current task once and returns the control to the executor scheduler.
pub async fn yield_now() {
    esp_idf_hal::task::yield_now().await;
}

/// Returns an async timer handle that can be used to create timers and delays.
///
/// # Panics
///
/// This function may panic if it fails to get access to the ESP timer service.
pub fn timer_handle() -> EspAsyncTimer {
    let hw_timer_service = HW_TIMER_SERVICE.get_or_init(|| EspTaskTimerService::new().unwrap());
    hw_timer_service.timer_async().unwrap()
}

/// Pauses the task execution for (at least) the specified amount of time.
///
/// # Panics
///
/// This function may panic if it fails to get access to the ESP timer service.
pub async fn sleep(duration: Duration) {
    let mut timer = timer_handle();
    timer.after(duration).await.unwrap();
}

/// Returns a sync timer handle that can be used to create timers and delays.
///
/// # Panics
///
/// This function may panic if it fails to get access to the ESP timer service.
#[must_use]
pub fn timer_handle_blocking() -> EspSyncTimer {
    EspSyncTimer::new()
}

/// Pauses the task execution for (at least) the specified amount of time.
///
/// Warning: This function blocks the entire thread, and it should really only be used for really short delays.
/// The async function [`sleep()`] should always be preferred when possible.
///
/// # Panics
///
/// This function may panic if it fails to get access to the ESP timer service.
pub fn sleep_blocking(duration: Duration) {
    let timer = EspSyncTimer::new();
    timer.after(duration).unwrap();
}

/// A wrapper over an [`EspTimer`] that implements a special `after()` method that blocks until the timer triggers.
pub struct EspSyncTimer {
    /// Internal sync hardware timer handle.
    timer: EspTimer<'static>,
    /// Receiver end of the channel used to signal timer events.
    rx: flume::Receiver<()>,
}

impl EspSyncTimer {
    /// Constructs a new instance of [`EspSyncTimer`].
    fn new() -> Self {
        let (tx, rx) = flume::bounded(1);
        let hw_timer_service = HW_TIMER_SERVICE.get_or_init(|| EspTaskTimerService::new().unwrap());
        let timer = hw_timer_service
            .timer(move || tx.send(()).unwrap())
            .unwrap();
        Self { timer, rx }
    }

    /// Blocks for at least `duration`, then returns.
    ///
    /// # Errors
    ///
    /// This method returns an error if it fails to initialize the timer.
    pub fn after(&self, duration: Duration) -> Result<(), EspError> {
        self.timer.after(duration)?;
        self.rx
            .recv()
            .map_err(|_| EspError::from_infallible::<-1>())?; // ESP_FAIL
        Ok(())
    }
}

/// Calls a blocking function from an async context without blocking executor worker threads.
/// Internally it works by spawning a new FreeRTOS thread, then asynchronously waiting for it to terminate.
///
/// # Errors
///
/// This function returns an error if something goes wrong while spawning the blocking function thread, or if the
/// communication of the result of `blocking_fn` fails.
///
/// # Panics
///
/// This function panics if it fails to spawn ins worker thread.
pub async fn call_blocking<F, T>(blocking_fn: F, stack_size: Option<usize>) -> Result<T, Error>
where
    F: (FnOnce() -> T) + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let handle = std::thread::Builder::new()
        .name("call_blocking_worker".to_owned())
        .stack_size(stack_size.unwrap_or(3 * 1024))
        .spawn(move || {
            let result = blocking_fn();
            tx.send(result).map_err(|_| Error::ParentCrash)
        })
        .unwrap();
    let result = rx.await.map_err(|_| Error::ChildCrash)?;
    handle.join().map_err(|_| Error::ChildCrash)??;
    Ok(result)
}

/// Errors related to async task management.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Panic or crash of the parent task or thread.
    /// Usually issued when a result cannot be returned to the parent/caller (e.g. with a channel whose receiveing end
    /// was dropped).
    #[error("parent task or thread paniced or crashed")]
    ParentCrash,
    /// Panic or crash of a child task or thread.
    #[error("child task or thread paniced or crashed")]
    ChildCrash,
}

//! A special kind of task that executes in a loop.

use super::TaskRepr;
use crate::{
    hal,
    task::{IAction, ITask, ITrigger, SharedMessageDispatcher, VcuResources},
};
use async_trait::async_trait;
use core::time::Duration;

// +------+
// | Task |
// +------+

/// A special kind of task that executes in a loop.
/// It features an optional timer in between consecutive iterations.
#[derive(Debug)]
pub struct Recurrent {
    /// Internal task representation.
    inner: TaskRepr<()>,
}

impl Recurrent {
    /// Constructs a new instance of [`Recurrent`].
    pub fn new<A>(name: &str, critical: bool, timer: Option<Duration>, action: A) -> Self
    where
        A: IAction<Input = ()> + 'static,
    {
        let trigger = Trigger::new(timer);
        let inner = TaskRepr::new(name, critical, trigger, action);
        Self { inner }
    }
}

#[async_trait]
impl ITask for Recurrent {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_critical(&self) -> bool {
        self.inner.is_critical()
    }

    async fn init(
        &mut self,
        _message_dispatcher: Option<SharedMessageDispatcher<'_>>,
        vcu_resources: VcuResources,
    ) {
        // `Recurrent` tasks are autonomous, i.e. they are triggered by external events (timers)
        // Therefore, they do not need to register callbacks on the `message_dispatcher`

        // Initialize action state
        self.inner.action().lock().await.init(vcu_resources).await;
    }

    async fn run(&self) {
        loop {
            self.inner.trigger().lock().await.condition().await;
            log::trace!("Task \"{}\" was triggered", self.name());
            self.inner.action().lock().await.run(()).await;
        }
    }
}

// +---------+
// | Trigger |
// +---------+

/// Trigger type for the [`Recurrent`] kind of task.
struct Trigger {
    /// Optional timer in between consecutive iterations.
    timer: Option<Duration>,
    /// Timer service handle.
    timer_handle: esp_idf_svc::timer::EspAsyncTimer,
}

impl Trigger {
    /// Constructs a new instance of [`Trigger`] with an optional timer.
    fn new(timer: Option<Duration>) -> Self {
        Self {
            timer,
            timer_handle: hal::task::timer_handle(),
        }
    }
}

#[async_trait]
impl ITrigger for Trigger {
    type Output = ();

    async fn condition(&mut self) -> Self::Output {
        // A very small sleep is necessary even if not provided to avoid triggering the task watchdog (TWDT)
        self.timer_handle
            .after(self.timer.unwrap_or(Duration::from_millis(1)))
            .await
            .unwrap();
    }
}

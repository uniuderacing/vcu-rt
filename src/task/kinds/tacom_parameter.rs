//! A special kind of task that is triggered upon update of a Tacom callback parameter.

use super::TaskRepr;
use crate::task::{IAction, ITask, ITrigger, SharedMessageDispatcher, VcuResources};
use async_trait::async_trait;
use core::fmt::{Debug, Display};
use tacom::{
    parameter::Parameterizable, slave::parameter::callback::Parameter as CallbackParameter,
};

// +------+
// | Task |
// +------+

/// A special kind of task that is triggered upon update of a Tacom callback parameter.
#[derive(Debug)]
pub struct TacomParameter<T>
where
    T: Parameterizable + Send + Sync + Copy + Debug + Display + 'static,
{
    /// Internal task representation.
    inner: TaskRepr<T>,
    /// Sender end of the channel used internally to queue value updates of the given Tacom parameter.
    /// Even if it is not used, this is stored here to avoid the trigger task failing due to a dropped channel.
    _queue: flume::Sender<T>,
}

impl<T> TacomParameter<T>
where
    T: Parameterizable + Send + Sync + Copy + Debug + Display + 'static,
{
    /// Constructs a new instance of [`TacomParameter`].
    /// The Tacom parameter given to this function is used to add a `on_update()` callback on it that triggers this
    /// task. After that, the parameter is returned alongside the task for further customization and usage.
    pub fn new<E, A>(
        name: &str,
        critical: bool,
        parameter: CallbackParameter<T, E>,
        action: A,
    ) -> (Self, CallbackParameter<T, E>)
    where
        E: Debug,
        A: IAction<Input = T> + 'static,
    {
        // The upper limit of this queue is quite low because we expect that parameter updates are not that frequent.
        // If this many parameter updates have been received, the sender will block, potentially allowing the receiver
        // to have more chances to be chosen by the scheduler, run, and thus free some space in the queue.
        let (tx, rx) = flume::bounded(16);

        let tx_2 = tx.clone();
        let task_name = name.to_owned();
        let parameter = parameter.on_update(move |new_value| {
            tx_2.send(new_value)
                .unwrap_or_else(|err| {
                    log::error!("Failed to trigger task \"{task_name}\" after the associated Tacom parameter was updated: {err}");
                });
        });

        let trigger = Trigger::new(rx);
        let inner = TaskRepr::new(name, critical, trigger, action);
        (Self { inner, _queue: tx }, parameter)
    }
}

#[async_trait]
impl<T> ITask for TacomParameter<T>
where
    T: Parameterizable + Send + Sync + Copy + Debug + Display + 'static,
{
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
        // `TacomParameter` tasks are autonomous, i.e. they are triggered by external events (Tacom parameter updates)
        // Therefore, they do not need to register callbacks on the `message_dispatcher`

        // Initialize action state
        self.inner.action().lock().await.init(vcu_resources).await;
    }

    async fn run(&self) {
        loop {
            let new_value = self.inner.trigger().lock().await.condition().await;
            log::trace!("Task \"{}\" was triggered", self.name());
            self.inner.action().lock().await.run(new_value).await;
        }
    }
}

// +---------+
// | Trigger |
// +---------+

/// Trigger type for the [`TacomParameter`] kind of task.
struct Trigger<T>
where
    T: Parameterizable + Send + Sync + Copy + Debug + Display + 'static,
{
    /// Receiver end of the channel used internally to queue value updates of the given Tacom parameter.
    queue: flume::Receiver<T>,
}

impl<T> Trigger<T>
where
    T: Parameterizable + Send + Sync + Copy + Debug + Display + 'static,
{
    /// Constructs a new instance of [`Trigger`].
    const fn new(queue: flume::Receiver<T>) -> Self {
        Self { queue }
    }
}

#[async_trait]
impl<T> ITrigger for Trigger<T>
where
    T: Parameterizable + Send + Sync + Copy + Debug + Display + 'static,
{
    type Output = T;

    async fn condition(&mut self) -> Self::Output {
        self.queue.recv_async().await.unwrap()
    }
}

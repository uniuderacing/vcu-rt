//! A special kind of task that is triggered upon reception of some specific type of message on the CAN bus.

use super::TaskRepr;
use crate::task::{IAction, ITask, ITrigger, SharedMessageDispatcher, VcuResources};
use async_trait::async_trait;
use core::fmt::Debug;

// +------+
// | Task |
// +------+

/// A special kind of task that is triggered upon reception of some specific type of message on the CAN bus.
#[derive(Debug)]
pub struct CanMessage<M>
where
    M: messages::message_struct::MessageStruct + Debug + Send + 'static,
{
    /// Internal task representation.
    inner: TaskRepr<M>,
    /// Sender end of the channel used internally to queue received CAN messages of the selected type.
    queue: flume::Sender<M>,
}

impl<M> CanMessage<M>
where
    M: messages::message_struct::MessageStruct + Debug + Send + 'static,
{
    /// Constructs a new instance of [`CanMessage`].
    pub fn new<A>(name: &str, critical: bool, action: A) -> Self
    where
        A: IAction<Input = M> + 'static,
    {
        // The upper limit of this queue is quite low because usually the messages that we want to listen on are not
        // that frequent.
        // Additionally, if this many messages have already been received and queued, the sender will block, potentially
        // allowing the receiver to have more chances to be chosen by the scheduler, run, and thus free some space in
        // the queue.
        let (tx, rx) = flume::bounded(16);
        let trigger = Trigger::new(rx);
        let inner = TaskRepr::new(name, critical, trigger, action);
        Self { inner, queue: tx }
    }
}

#[async_trait]
impl<M> ITask for CanMessage<M>
where
    M: messages::message_struct::MessageStruct + Debug + Send + 'static,
{
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn is_critical(&self) -> bool {
        self.inner.is_critical()
    }

    async fn init(
        &mut self,
        message_dispatcher: Option<SharedMessageDispatcher<'_>>,
        vcu_resources: VcuResources,
    ) {
        // Register a callback for messages of type `M` on the `message_dispatcher`
        if let Some(message_dispatcher) = message_dispatcher {
            let task_name = self.name().to_owned();
            let queue = self.queue.clone();
            let mut guard = message_dispatcher.lock().await;
            let old_dispatcher = guard.take().unwrap(); // Safe due to `message_dispatcher` invariant
            let new_dispatcher = old_dispatcher.attach_callback(move |message: M| {
                queue.send(message).unwrap_or_else(|err| {
                    log::error!("Failed to trigger task \"{task_name}\" after a relevant CAN message was received: {err}");
                });
            });
            *guard = Some(new_dispatcher);
            drop(guard); // Invariant of `mutex_dispatcher` is preserved (see previous line)
        } else {
            log::error!(
                "Task \"{}\" is triggered by some CAN message type, but no CAN bus driver was provided to the runtime that it is bound to",
                self.name(),
            );
        }

        // Initialize action state
        self.inner.action().lock().await.init(vcu_resources).await;
    }

    async fn run(&self) {
        loop {
            let message = self.inner.trigger().lock().await.condition().await;
            log::trace!("Task \"{}\" was triggered", self.name());
            self.inner.action().lock().await.run(message).await;
        }
    }
}

// +---------+
// | Trigger |
// +---------+

/// Trigger type for the [`CanMessage`] kind of task.
struct Trigger<M>
where
    M: messages::message_struct::MessageStruct + Debug + Send + 'static,
{
    /// Receiver end of the channel used internally to queue received CAN messages of the selected type.
    queue: flume::Receiver<M>,
}

impl<M> Trigger<M>
where
    M: messages::message_struct::MessageStruct + Debug + Send + 'static,
{
    /// Constructs a new instance of [`Trigger`].
    const fn new(queue: flume::Receiver<M>) -> Self {
        Self { queue }
    }
}

#[async_trait]
impl<M> ITrigger for Trigger<M>
where
    M: messages::message_struct::MessageStruct + Debug + Send + 'static,
{
    type Output = M;

    async fn condition(&mut self) -> Self::Output {
        self.queue.recv_async().await.unwrap()
    }
}

//! A special kind of task whose trigger is specified by the user.

use super::TaskRepr;
use crate::task::{IAction, ITask, ITrigger, SharedMessageDispatcher, VcuResources};
use async_trait::async_trait;
use core::fmt::Debug;

// +------+
// | Task |
// +------+

/// A special kind of task whose trigger is specified by the user.
#[derive(Debug)]
pub struct UserTrigger<IO>
where
    IO: Debug + Send + 'static,
{
    /// Internal task representation.
    inner: TaskRepr<IO>,
}

impl<IO> UserTrigger<IO>
where
    IO: Debug + Send + 'static,
{
    /// Constructs a new instance of [`UserTrigger`].
    pub fn new<T, A>(name: &str, critical: bool, trigger: T, action: A) -> Self
    where
        T: ITrigger<Output = IO> + 'static,
        A: IAction<Input = IO> + 'static,
    {
        let inner = TaskRepr::new(name, critical, trigger, action);
        Self { inner }
    }
}

#[async_trait]
impl<IO> ITask for UserTrigger<IO>
where
    IO: Debug + Send + 'static,
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
        // `UserTrigger` task triggers are managed by the user, not by the `Runtime`

        // Initialize action state
        self.inner.action().lock().await.init(vcu_resources).await;
    }

    async fn run(&self) {
        loop {
            let data = self.inner.trigger().lock().await.condition().await;
            log::trace!("Task \"{}\" was triggered", self.name());
            self.inner.action().lock().await.run(data).await;
        }
    }
}

//! Specialized kinds of tasks with common types of triggers.

mod can_message;
mod recurrent;
mod tacom_parameter;
mod user_trigger;
pub use {can_message::*, recurrent::*, tacom_parameter::*, user_trigger::*};

use super::{IAction, ITrigger};
use core::fmt::{self, Debug};
use futures::lock::Mutex as AsyncMutex;
use std::sync::Arc;

/// Common internal task representation.
struct TaskRepr<IO> {
    /// The name of the task.
    name: String,
    /// Whether the task is critical or not.
    critical: bool,
    /// Event that triggers the task.
    trigger: Arc<AsyncMutex<dyn ITrigger<Output = IO>>>,
    /// Action that is executed upon triggering.
    action: Arc<AsyncMutex<dyn IAction<Input = IO>>>,
}

impl<IO> TaskRepr<IO> {
    /// Constructs a new instance of [`TaskRepr`].
    fn new<T, A>(name: &str, critical: bool, trigger: T, action: A) -> Self
    where
        T: ITrigger<Output = IO> + 'static,
        A: IAction<Input = IO> + 'static,
    {
        let arc_trigger = Arc::new(AsyncMutex::new(trigger));
        let arc_action = Arc::new(AsyncMutex::new(action));

        Self {
            name: name.to_owned(),
            critical,
            trigger: arc_trigger,
            action: arc_action,
        }
    }

    /// Returns the name of this task.
    const fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Returns true if and only if the task is critical.
    const fn is_critical(&self) -> bool {
        self.critical
    }

    /// Returns a reference to the trigger of this task.
    fn trigger(&self) -> Arc<AsyncMutex<dyn ITrigger<Output = IO>>> {
        self.trigger.clone()
    }

    /// Returns a reference to the action implemented by this task.
    fn action(&self) -> Arc<AsyncMutex<dyn IAction<Input = IO>>> {
        self.action.clone()
    }
}

impl<IO> Debug for TaskRepr<IO> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskRepr")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

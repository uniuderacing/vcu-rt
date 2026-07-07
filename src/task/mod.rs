//! Representation of tasks that can be run by a [`Runtime`](crate::Runtime) instance.

pub(crate) mod internal;
mod kinds;
pub use kinds::*;

use crate::param::{SharedMessageDispatcher, VcuResources};
use async_trait::async_trait;
use core::fmt::Debug;

/// Abstract representation of a task.
///
/// Note: implementors of this trait must use the `async_trait` macro (automatically re-exported by the `prelude`
/// module. This is necessary to make this trait object-safe.
#[allow(clippy::module_name_repetitions)]
#[async_trait]
pub trait ITask: Debug + Send {
    /// Returns the name of the task.
    fn name(&self) -> &str;

    /// Returns true if and only if the task is critical.
    fn is_critical(&self) -> bool;

    /// Initializes the task with som stuff provided by the runtime.
    /// Typical operations include:
    /// - using Tacom's `message_dispatcher` to attach any callback needed by the task;
    /// - initializing the action state with the given VCU resources.
    /// You can safely assume that this method gets called before any invocation of `run()`.
    ///
    /// Invariant for `message_dispatcher`: if the mutex is unlocked, then the enclosed option must be `Some(_)`.
    async fn init(
        &mut self,
        message_dispatcher: Option<SharedMessageDispatcher<'_>>,
        vcu_resources: VcuResources,
    );

    /// Runs the task.
    /// This usually consists in awaiting for the trigger and then executing the action, possibly in a loop.
    async fn run(&self);
}

/// Abstract representation of the event that triggers the execution of a task action.
///
/// Note: implementors of this trait must use the `async_trait` macro (automatically re-exported by the `prelude`
/// module. This is necessary to make this trait object-safe.
#[async_trait]
pub trait ITrigger: Send {
    /// Optional output value of the trigger.
    /// It must match with the input of the corresponding action.
    type Output;

    /// Asynchronous function which returns as soon as the trigger is triggered.
    async fn condition(&mut self) -> Self::Output;
}

/// Abstract representation of the action carried out by a task.
/// Structs implementing this trait can have fields to store internal state, which gets stored across subsequent
/// triggers.
///
/// Note: implementors of this trait must use the `async_trait` macro (automatically re-exported by the `prelude`
/// module. This is necessary to make this trait object-safe.
#[async_trait]
pub trait IAction: Send {
    /// Optional input of the action invocation.
    /// It must match with the output of the corresponding trigger.
    type Input;

    /// Initializes the action's state with some stuff provided by the runtime.
    /// You can safely assume that this method gets called before any invocation of `run()`.
    async fn init(&mut self, vcu_resources: VcuResources);

    /// Asynchronously runs the action to be carried out upon triggering.
    async fn run(&mut self, input: Self::Input);
}

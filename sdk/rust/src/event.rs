//! Connection state and callbacks. Callbacks run on whichever SDK thread
//! observed the change; they must not block.

use crate::error::Error;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Error = 3,
}

pub trait EventHandler: Send + Sync {
    fn state(&self, _state: ConnectionState) {}
    fn status(&self, _status: &input_protocol::v3::Status) {}
    fn error(&self, _error: &Error) {}
}

impl EventHandler for () {}

pub(crate) fn notify(
    handler: &Option<Arc<dyn EventHandler>>,
    event: impl FnOnce(&dyn EventHandler),
) {
    if let Some(handler) = handler {
        event(handler.as_ref());
    }
}

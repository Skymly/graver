//! User-session process that owns composition state.
//!
//! This is not a Windows Service Control Manager service. Each named-pipe
//! client gets its own engine. The in-process text service must not link this
//! crate into the host application.

mod dispatch;
mod pipe;
mod stdio;

pub use dispatch::dispatch;
pub use pipe::{serve_pipe, serve_pipe_once};
pub use stdio::serve_stream;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] graver_ipc::ProtocolError),
    #[error("windows api: {0}")]
    Windows(String),
    #[error("client disconnected")]
    Disconnected,
}

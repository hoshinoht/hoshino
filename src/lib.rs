#![forbid(unsafe_code)]

mod app;
pub mod cli;
pub mod collect;
pub mod config;
pub mod hooks;
pub mod limits;
pub mod model;
mod regular_file;
pub mod render;

/// Entry point shared by the `hoshino` binary and future command modules.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    app::run().map_err(Into::into)
}

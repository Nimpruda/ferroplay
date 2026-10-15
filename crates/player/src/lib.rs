//! A tiny video player core.
//!
//! The whole design is one loop:
//!
//! ```text
//! Event ──▶ State.apply(fact) ──▶ plugins ──▶ Intents ──▶ State.resolve ──▶ Cmds ──▶ host
//! ```
//!
//! - **Facts** come from the host (the media element) and are the only thing that changes state.
//! - **Intents** are wishes ("seek +5s") from the UI or plugins.
//! - **Cmds** are absolute instructions ("seek to 42.0") for the host to run.
//!
//! No I/O and no DOM, so it runs (and is tested) anywhere; `no_std` keeps it small in wasm.

#![no_std]
extern crate alloc;

mod player;
pub mod plugins;
mod state;
mod view;

pub use player::{Event, Player, Plugin};
pub use state::{Cmd, Fact, Intent, State};
pub use view::View;

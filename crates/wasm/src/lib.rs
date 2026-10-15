//! The browser build: the player and its plugins behind the code-number-bytes interface that
//! `web/player.js` talks to. Only wasm32 builds produce something (`./build.sh`).

#![cfg_attr(target_arch = "wasm32", no_std)]

#[cfg(target_arch = "wasm32")]
extern crate alloc;

#[cfg(target_arch = "wasm32")]
mod ffi;

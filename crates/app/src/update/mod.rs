//! Checking GitHub Releases for a newer VoiceChanger, downloading, verifying and installing it.

pub mod client;
pub mod install;
pub mod release;
pub mod schedule;
pub mod state;
#[cfg(test)]
mod testserver;
pub mod ui;
pub mod verify;
pub mod version;
pub mod worker;

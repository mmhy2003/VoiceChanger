//! Checking GitHub Releases for a newer VoiceChanger, downloading, verifying and installing it.

pub mod client;
pub mod release;
pub mod schedule;
#[cfg(test)]
mod testserver;
pub mod verify;
pub mod version;
pub mod worker;

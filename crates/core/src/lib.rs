//! Everything between the recording engine and notizli.ch: pairing, the
//! device token, the queue of recordings waiting for upload, and the upload.

pub mod account;
pub mod api;
pub mod pairing;
pub mod store;

#[cfg(test)]
mod testserver;

//! The Rust core of Phonoscule for Android.
//!
//! The library scan, the play engine with its AAudio output, the queue and the saved session all
//! come from `phonoscule`, shared with the desktop players. This crate holds them for the life of
//! the process ([`model`]) and puts a request/response protocol over them ([`proto`]).
//!
//! The Kotlin shell does every bit of I/O on the listener's music, through the Storage Access
//! Framework, and hands the core what it asked for: directory listings, and open file descriptors
//! ([`saf`]). The core's own caches and session live at paths Kotlin gives it.

pub mod model;
pub mod proto;
pub mod saf;

// The JNI shim only means anything when a JVM loads the library. Leaving it out of host builds
// keeps `cargo test` free of the `jni` crate's dynamic-linking needs.
#[cfg(target_os = "android")]
mod jni;

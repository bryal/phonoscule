//! Internals of the phonoscule GUI, exposed as a library so benchmarks (and potentially tests)
//! can reach them; `main.rs` holds the iced application glue.

pub mod album_grid;
pub mod background;
pub mod conf;
pub mod coverflow;
pub mod library;
pub mod media;
pub mod player;
pub mod playlist;
pub mod watcher;

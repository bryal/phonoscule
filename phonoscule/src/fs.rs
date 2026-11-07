//! Where the music is read from: listing directories and opening files.
//!
//! The [`library`](crate::library) scan and the [`player`](crate::player) engine never touch the
//! filesystem themselves. They ask an [`Fs`], so a player whose music sits behind something other
//! than plain paths can still use them. On a desktop that is [`Local`]. On Android it is the
//! Storage Access Framework, where the platform lists a directory and hands back an open file
//! descriptor, and paths are only names within the tree the listener picked.
//!
//! The caches and the saved session are not music, and are still read and written by path.
//!
//! Wants std.

use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    time::SystemTime,
};

/// What an [`Fs`] method returns: a boxed future, so that `dyn Fs` is a type.
pub type IoFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The directory's path joined with the entry's name.
    pub path: PathBuf,
    pub is_dir: bool,
    /// When the file last changed. Meaningless for a directory.
    pub mtime: SystemTime,
    /// In bytes. Meaningless for a directory.
    pub size: u64,
}

/// A tree of directories and files to read music from.
pub trait Fs: Send + Sync {
    /// Every directory in `dir`, and every file in it that is `wanted`, in no particular order.
    fn read_dir<'a>(&'a self, dir: &'a Path, wanted: fn(&Path) -> bool) -> IoFuture<'a, Vec<DirEntry>>;

    /// `path`, opened for reading. The file must be seekable.
    fn open<'a>(&'a self, path: &'a Path) -> IoFuture<'a, std::fs::File>;

    /// The name another program on this machine would know `path` by, such as the absolute path of
    /// a cover handed to the desktop's now-playing widget. `path` itself if there is none.
    fn canonicalize<'a>(&'a self, path: &'a Path) -> IoFuture<'a, PathBuf> {
        Box::pin(async move { Ok(path.to_path_buf()) })
    }
}

/// The local filesystem, through `std::fs`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Local;

impl Fs for Local {
    fn read_dir<'a>(&'a self, dir: &'a Path, wanted: fn(&Path) -> bool) -> IoFuture<'a, Vec<DirEntry>> {
        Box::pin(smol::unblock({
            let dir = dir.to_path_buf();
            move || {
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(&dir)? {
                    let Ok(entry) = entry else { continue };
                    let Ok(file_type) = entry.file_type() else { continue };
                    let path = entry.path();
                    if file_type.is_dir() {
                        entries.push(DirEntry { path, is_dir: true, mtime: SystemTime::UNIX_EPOCH, size: 0 });
                        continue;
                    }
                    if !wanted(&path) {
                        continue;
                    }
                    // Through a symlink, as opening it would. A link to a directory is not walked,
                    // which keeps a link cycle from walking forever.
                    let Ok(meta) = std::fs::metadata(&path) else { continue };
                    if meta.is_dir() {
                        continue;
                    }
                    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    entries.push(DirEntry { path, is_dir: false, mtime, size: meta.len() });
                }
                Ok(entries)
            }
        }))
    }

    fn open<'a>(&'a self, path: &'a Path) -> IoFuture<'a, std::fs::File> {
        let path = path.to_path_buf();
        Box::pin(smol::unblock(move || std::fs::File::open(path)))
    }

    fn canonicalize<'a>(&'a self, path: &'a Path) -> IoFuture<'a, PathBuf> {
        Box::pin(smol::fs::canonicalize(path))
    }
}

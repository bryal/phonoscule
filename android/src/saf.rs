//! The music, as Kotlin reads it through the Storage Access Framework.
//!
//! [`Saf`] is the [`Fs`] the library scan and the play engine read through. Each listing or open
//! becomes an [`Op`] that waits in a queue until a Kotlin worker takes it with [`Saf::next_op`],
//! performs it, and hands back the [`Answer`] with [`Saf::answer`]. So Kotlin still does every bit
//! of I/O on the listener's music, and nothing here calls into the JVM.
//!
//! Paths are names within the folder the listener picked: `/` is that folder, and
//! `/Artist/Album/01.opus` a file two directories down. Kotlin finds the document by walking the
//! names.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use futures::channel::oneshot;
use phonoscule::fs::{DirEntry, Fs, IoFuture};
use serde::{Deserialize, Serialize};
use smol::channel;

/// The root of the picked folder.
pub const ROOT: &str = "/";

/// One piece of I/O for Kotlin to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// List the directory at `path`, answering [`Answer::Listing`].
    ReadDir { id: u64, path: String },
    /// Open the file at `path` for reading, answering [`Answer::Opened`].
    Open { id: u64, path: String },
}

impl Op {
    fn id(&self) -> u64 {
        match self {
            Op::ReadDir { id, .. } | Op::Open { id, .. } => *id,
        }
    }
}

/// How an [`Op`] went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    Listing {
        entries: Vec<Entry>,
    },
    /// A file descriptor open for reading, which this side now owns and closes.
    Opened {
        fd: i32,
    },
    Failed {
        message: String,
    },
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    /// Milliseconds since the Unix epoch.
    pub mtime: i64,
    /// In bytes.
    pub size: u64,
}

pub struct Saf {
    ops: (channel::Sender<Op>, channel::Receiver<Op>),
    /// Who is waiting on each op Kotlin has not answered yet.
    waiting: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
    next_id: AtomicU64,
}

impl Saf {
    pub fn new() -> Self {
        Saf { ops: channel::unbounded(), waiting: Mutex::new(HashMap::new()), next_id: AtomicU64::new(0) }
    }

    /// The next op to perform, waiting for one if there is none.
    pub fn next_op(&self) -> Op {
        self.ops.1.recv_blocking().expect("the sender lives in self")
    }

    /// Hands back what the op numbered `id` came to. An answer nobody is waiting for any more is
    /// dropped, closing any file it opened.
    pub fn answer(&self, id: u64, answer: Answer) {
        let waiter = self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        match waiter {
            Some(waiter) => {
                if let Err(Answer::Opened { fd }) = waiter.send(answer) {
                    drop(unsafe { owned_file(fd) });
                }
            }
            None => {
                log::warn!("an answer to op {id}, which nobody asked for");
                if let Answer::Opened { fd } = answer {
                    drop(unsafe { owned_file(fd) });
                }
            }
        }
    }

    async fn ask(&self, op: impl FnOnce(u64) -> Op) -> io::Result<Answer> {
        let op = op(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(op.id(), tx);
        self.ops.0.send(op).await.map_err(io::Error::other)?;
        match rx.await {
            Ok(Answer::Failed { message }) => Err(io::Error::other(message)),
            Ok(answer) => Ok(answer),
            Err(_) => Err(io::Error::other("the op was dropped unanswered")),
        }
    }
}

impl Default for Saf {
    fn default() -> Self {
        Self::new()
    }
}

impl Fs for Saf {
    fn read_dir<'a>(&'a self, dir: &'a Path) -> IoFuture<'a, Vec<DirEntry>> {
        Box::pin(async move {
            let path = name(dir)?;
            match self.ask(|id| Op::ReadDir { id, path }).await? {
                Answer::Listing { entries } => Ok(entries
                    .into_iter()
                    .map(|entry| DirEntry {
                        path: dir.join(&entry.name),
                        is_dir: entry.dir,
                        mtime: SystemTime::UNIX_EPOCH + Duration::from_millis(entry.mtime.max(0) as u64),
                        size: entry.size,
                    })
                    .collect()),
                other => Err(unexpected(other)),
            }
        })
    }

    fn open<'a>(&'a self, path: &'a Path) -> IoFuture<'a, std::fs::File> {
        Box::pin(async move {
            let path = name(path)?;
            match self.ask(|id| Op::Open { id, path }).await? {
                Answer::Opened { fd } => Ok(unsafe { owned_file(fd) }),
                other => Err(unexpected(other)),
            }
        })
    }
}

/// `path` as Kotlin is sent it.
fn name(path: &Path) -> io::Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| io::Error::other(format!("{path:?} is not UTF-8")))
}

fn unexpected(answer: Answer) -> io::Error {
    io::Error::other(format!("answered with the wrong kind: {answer:?}"))
}

/// Takes ownership of a file descriptor Kotlin opened.
///
/// # Safety
/// `fd` must be open, and owned by nobody else: Kotlin detached it from its `ParcelFileDescriptor`.
unsafe fn owned_file(fd: i32) -> std::fs::File {
    use std::os::fd::FromRawFd;
    unsafe { std::fs::File::from_raw_fd(fd) }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use std::os::fd::IntoRawFd;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Plays Kotlin's part over `root` on the host: a thread answering every op from the local
    /// filesystem, the way the app's workers answer them from SAF.
    pub fn serve(saf: Arc<Saf>, root: PathBuf) {
        std::thread::spawn(move || {
            loop {
                let op = saf.next_op();
                let local = |path: &str| root.join(path.trim_start_matches('/'));
                let (id, answer) = match op {
                    Op::ReadDir { id, path } => (id, list(&local(&path))),
                    Op::Open { id, path } => match std::fs::File::open(local(&path)) {
                        Ok(file) => (id, Answer::Opened { fd: file.into_raw_fd() }),
                        Err(e) => (id, Answer::Failed { message: e.to_string() }),
                    },
                };
                saf.answer(id, answer);
            }
        });
    }

    fn list(dir: &Path) -> Answer {
        let read = || -> io::Result<Vec<Entry>> {
            let mut entries = vec![];
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let meta = entry.metadata()?;
                let mtime = meta.modified()?.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis();
                entries.push(Entry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    dir: meta.is_dir(),
                    mtime: mtime as i64,
                    size: meta.len(),
                });
            }
            Ok(entries)
        };
        match read() {
            Ok(entries) => Answer::Listing { entries },
            Err(e) => Answer::Failed { message: e.to_string() },
        }
    }

    #[test]
    fn lists_and_opens_through_the_answers() {
        let root = std::env::temp_dir().join(format!("phonoscule-android-saf-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Artist")).unwrap();
        std::fs::write(root.join("Artist/a.opus"), b"hello").unwrap();

        let saf = Arc::new(Saf::new());
        serve(Arc::clone(&saf), root.clone());
        smol::block_on(async {
            let entries = saf.read_dir(Path::new(ROOT)).await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].path, Path::new("/Artist"));
            assert!(entries[0].is_dir);

            let mut file = saf.open(Path::new("/Artist/a.opus")).await.unwrap();
            let mut read = String::new();
            io::Read::read_to_string(&mut file, &mut read).unwrap();
            assert_eq!(read, "hello");

            assert!(saf.open(Path::new("/nonesuch")).await.is_err());
        });
        std::fs::remove_dir_all(&root).unwrap();
    }
}

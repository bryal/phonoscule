//! Where the program's terminal output goes on Windows, which depends on how it was started.
//!
//! The binary is linked as a Windows subsystem application (see the attribute in `main.rs`), so
//! double-clicking it in Explorer, or launching it from a shortcut or the Start menu, does not
//! conjure a console window at it. On its own that would also lose the terminal output - the help,
//! the version, the log - for anyone who did start it from a shell, so [`prepare`] adopts the
//! console it was launched from when there is one.
//!
//! When there is not, the standard handles are left invalid, and that is worse than merely useless:
//! `println!` panics on an invalid handle, and this player logs at startup, so a double-clicked
//! build would die printing its first line. So they are pointed at `NUL` instead, and every printing
//! path in the program carries on working without knowing the difference.
//!
//! Everywhere but Windows this is all inapplicable: a process inherits whatever its parent gave it.

/// Whether anything can see what the program prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// A console, or a redirect to a file or a pipe: printing reaches somewhere readable.
    Live,
    /// Nothing was attached, so printing goes to `NUL`. A message meant for a person has to reach
    /// them some other way - see [`alert`].
    ///
    /// Only Windows can end up here; the variant stays unconditional so the code that acts on this
    /// reads the same everywhere rather than growing a `cfg` of its own.
    #[cfg_attr(not(windows), expect(dead_code, reason = "only the Windows `prepare` constructs it"))]
    Discarded,
}

/// Settles where output goes, before anything is printed.
#[cfg(not(windows))]
pub fn prepare() -> Output {
    Output::Live
}

/// Reports a message that must not be missed: to stderr when there is somewhere to print, and
/// otherwise, since the program was started from a graphical shell, in a dialog.
#[cfg(not(windows))]
pub fn alert(_title: &str, message: &str) {
    eprint!("{message}");
}

#[cfg(windows)]
pub use windows_impl::{alert, prepare};

#[cfg(windows)]
mod windows_impl {
    use super::Output;
    use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_WRITE};
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::{HSTRING, PCWSTR, w};

    pub fn prepare() -> Output {
        // What we were handed before touching anything. A redirect (`> log.txt`) supplies these even
        // to a subsystem application with no console of its own, and adopting one must not take them
        // away.
        let inherited = [(STD_OUTPUT_HANDLE, usable(STD_OUTPUT_HANDLE)), (STD_ERROR_HANDLE, usable(STD_ERROR_HANDLE))];

        // Adopt the console of whatever launched us. This is what keeps a run from an existing
        // terminal printing where it always did; it fails when there is no console to adopt, which is
        // exactly the Explorer case the subsystem attribute is there for.
        let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok();

        // Attaching repoints the standard handles at the console it attached to, redirected ones
        // included, which would quietly swallow the `> log.txt` somebody asked for. Put back
        // everything that was already good, and let the console keep only what was not.
        for (id, handle) in inherited {
            if let Some(handle) = handle {
                let _ = unsafe { SetStdHandle(id, handle) };
            }
        }

        if !attached {
            // Nothing was attached, so whatever is still without a destination has none to be had.
            // Rust's `println!` panics on an invalid handle rather than shrugging, and this player
            // logs before it draws a thing, so point those at the null device. Failing to open it is
            // not worth handling: it only leaves us where we already were.
            if let Ok(nul) = nul() {
                for (id, handle) in inherited {
                    if handle.is_none() {
                        let _ = unsafe { SetStdHandle(id, nul) };
                    }
                }
            }
        }

        // Whether a message meant for a person will actually reach one, which is stderr's business.
        if attached || inherited[1].1.is_some() { Output::Live } else { Output::Discarded }
    }

    /// The standard handle, if it is one we can actually write to. A subsystem application with no
    /// console gets null ones, and a handle can also be explicitly invalid.
    fn usable(id: STD_HANDLE) -> Option<HANDLE> {
        match unsafe { GetStdHandle(id) } {
            Ok(handle) if !handle.is_invalid() && handle != INVALID_HANDLE_VALUE => Some(handle),
            _ => None,
        }
    }

    /// The null device, opened for writing.
    fn nul() -> windows::core::Result<HANDLE> {
        unsafe {
            windows::Win32::Storage::FileSystem::CreateFileW(
                w!("NUL"),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_WRITE,
                None,
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
    }

    pub fn alert(title: &str, message: &str) {
        let (title, message) = (HSTRING::from(title), HSTRING::from(message));
        // No owner window: this is for failures during startup, before there is one.
        unsafe { MessageBoxW(None, PCWSTR(message.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR) };
    }
}

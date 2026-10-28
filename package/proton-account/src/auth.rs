// SPDX-License-Identifier: GPL-3.0-or-later
use super::{STOP, failure};
use muon::rt::{Monotonic, MuonSystemTime, SinceUnixEpoch, SystemTimeFactory};
use std::{
    future::Future,
    io::{self, Write},
    os::fd::AsRawFd,
    pin::Pin,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
pub(super) struct Clock(Instant);
impl Default for Clock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl muon::rt::Sleep for Clock {
    type Sleep<'a>
        = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'a>>
    where
        Self: 'a;
    fn sleep(&self, duration: Duration) -> Self::Sleep<'static> {
        Box::pin(tokio::time::sleep(duration))
    }
}
impl muon::rt::InstantFactory for Clock {
    type Instant = muon::rt::MuonInstant;
    fn now(&self) -> Self::Instant {
        muon::rt::MuonInstant::from_duration(self.0.elapsed())
    }
}
// Instant never goes backwards and shares its epoch across clones.
unsafe impl Monotonic for Clock {}
impl SystemTimeFactory for Clock {
    type SystemTime = MuonSystemTime;
    fn now(&self) -> Self::SystemTime {
        MuonSystemTime::since_unix_epoch(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default(),
        )
    }
}
#[derive(Clone, Debug)]
pub(super) struct Executor;
impl muon::rt::Spawn for Executor {
    fn spawn_obj(
        &self,
        future: muon::rt::FutureObj<'static, ()>,
    ) -> Result<(), muon::rt::SpawnError> {
        tokio::spawn(future);
        Ok(())
    }
}

struct Terminal {
    file: std::fs::File,
    original: libc::termios,
}
impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.original) };
    }
}
pub(super) fn prompt(label: &str, secret: bool) -> io::Result<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut original) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut terminal = Terminal { file, original };
    terminal.file.write_all(label.as_bytes())?;
    terminal.file.flush()?;
    if secret {
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ECHONL);
        if unsafe { libc::tcsetattr(terminal.file.as_raw_fd(), libc::TCSAFLUSH, &hidden) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let mut bytes = Vec::new();
    loop {
        let mut byte = 0u8;
        let n = unsafe { libc::read(terminal.file.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 || STOP.load(Ordering::Relaxed) {
            return Err(failure("Login interrupted"));
        }
        if byte == b'\n' {
            break;
        }
        if bytes.len() >= 4096 {
            return Err(failure("Login input is too long"));
        }
        bytes.push(byte);
    }
    if secret {
        terminal.file.write_all(b"\n")?;
    }
    String::from_utf8(bytes).map_err(|_| failure("Login input must be UTF-8"))
}
pub(super) fn erase(value: &mut String) {
    // Overwriting with zero bytes preserves UTF-8 validity. The SDK may have its
    // own copies; this only erases the command's owned prompt buffer.
    unsafe { value.as_mut_vec().fill(0) };
}

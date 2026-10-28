// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::OpenOptions,
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::atomic::AtomicBool,
};
static STOP: AtomicBool = AtomicBool::new(false);
#[path = "openwrt-account.rs"]
mod account;
mod auth;
fn failure(message: &str) -> io::Error {
    io::Error::other(message)
}

// Open before checking metadata, reject symlinks, and bound reads. Neither JSON
// values nor private-key contents are included in error messages or logs.
fn read_file(path: &Path, secret: bool) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    let forbidden = if secret { 0o077 } else { 0o022 };
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & forbidden != 0
    {
        return Err(failure(
            "File must be owned by the current user and regular; key must be private; configuration must not be group/world writable",
        ));
    }
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes)?;
    if bytes.len() > 16_384 {
        return Err(failure("Configuration or key file exceeds size limit"));
    }
    Ok(bytes)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = account::main(&args, false) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

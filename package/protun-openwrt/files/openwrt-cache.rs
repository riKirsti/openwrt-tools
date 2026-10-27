// SPDX-License-Identifier: GPL-3.0-or-later
use super::failure;
use protun::api::connection::{CacheKey, Cookie, Cookies, ForkSelectorInfo, PersistentCache};
use serde::{Deserialize, Serialize};
use std::{
    ffi::CString,
    fs::{DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Inner {
    directory: File,
    failed: AtomicBool,
}
#[derive(Clone)]
pub struct DiskCache(Arc<Inner>);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub selector: String,
    pub cookies: Vec<SelectorCookie>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorCookie {
    pub name: String,
    pub value: String,
}
impl Selector {
    pub fn validate(&self) -> io::Result<()> {
        if self.selector.is_empty() || self.selector.len() > 16_384 || self.cookies.len() > 64 {
            return Err(failure("Invalid session fork selector"));
        }
        Ok(())
    }
    pub fn into_upstream(self) -> io::Result<ForkSelectorInfo> {
        self.validate()?;
        Ok(ForkSelectorInfo {
            selector: self.selector,
            cookies: Cookies {
                items: self
                    .cookies
                    .into_iter()
                    .map(|c| Cookie {
                        name: c.name,
                        value: c.value,
                    })
                    .collect(),
            },
        })
    }
}

impl DiskCache {
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            DirBuilder::new().mode(0o700).create(path)?;
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_DIRECTORY)
            .open(path)?;
        let m = directory.metadata()?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err(failure(
                "Session cache must be an owned private directory (0700)",
            ));
        }
        Ok(Self(Arc::new(Inner {
            directory,
            failed: AtomicBool::new(false),
        })))
    }
    pub fn failed(&self) -> bool {
        self.0.failed.load(Ordering::Relaxed)
    }
    fn mark_error(&self) {
        self.0.failed.store(true, Ordering::Relaxed);
    }
    fn name(key: CacheKey) -> &'static str {
        match key {
            CacheKey::Certificate => "certificate.bin",
            CacheKey::PrivateKey => "private-key.bin",
            CacheKey::ApiSession => "session.bin",
        }
    }
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let name = CString::new(name).unwrap();
        let fd = unsafe {
            libc::openat(
                self.0.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let m = file.metadata()?;
        if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err(failure(
                "Session cache file has unsafe ownership or permissions",
            ));
        }
        let mut bytes = Vec::new();
        file.take(1_048_577).read_to_end(&mut bytes)?;
        if bytes.len() > 1_048_576 {
            return Err(failure("Session cache entry is too large"));
        }
        Ok(Some(bytes))
    }
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > 1_048_576 {
            return Err(failure("Session cache entry is too large"));
        }
        let temporary = CString::new(format!(
            ".write-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap();
        let name = CString::new(name).unwrap();
        let dir = self.0.directory.as_raw_fd();
        let fd = unsafe {
            libc::openat(
                dir,
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::mode_t,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(bytes)?;
            file.sync_all()?;
            if unsafe { libc::renameat(dir, temporary.as_ptr(), dir, name.as_ptr()) } < 0 {
                return Err(io::Error::last_os_error());
            }
            self.0.directory.sync_all()
        })();
        if result.is_err() {
            unsafe { libc::unlinkat(dir, temporary.as_ptr(), 0) };
        }
        result
    }
    fn delete(&self, name: &str) -> io::Result<()> {
        let name = CString::new(name).unwrap();
        if unsafe { libc::unlinkat(self.0.directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        self.0.directory.sync_all()
    }
    pub fn save_selector(&self, selector: &Selector) -> io::Result<()> {
        selector.validate()?;
        self.write(
            "fork.json",
            &serde_json::to_vec(selector).map_err(|_| failure("Cannot encode fork selector"))?,
        )?;
        self.delete("session.bin")?;
        self.delete("certificate.bin")?;
        Ok(())
    }
    pub fn take_selector(&self) -> io::Result<Option<ForkSelectorInfo>> {
        let Some(bytes) = self.read("fork.json")? else {
            return Ok(None);
        };
        let selector: Selector =
            serde_json::from_slice(&bytes).map_err(|_| failure("Invalid cached fork selector"))?;
        let selector = selector.into_upstream()?;
        self.delete("fork.json")?;
        Ok(Some(selector))
    }
    pub fn clear(&self) -> io::Result<()> {
        for name in [
            "certificate.bin",
            "private-key.bin",
            "session.bin",
            "fork.json",
        ] {
            self.delete(name)?;
        }
        Ok(())
    }
}
impl PersistentCache for DiskCache {
    fn put(&self, key: CacheKey, bytes: Vec<u8>) {
        if self.write(Self::name(key), &bytes).is_err() {
            self.mark_error()
        }
    }
    fn get(&self, key: CacheKey) -> Option<Vec<u8>> {
        match self.read(Self::name(key)) {
            Ok(v) => v,
            Err(_) => {
                self.mark_error();
                None
            }
        }
    }
    fn remove(&self, key: CacheKey) {
        if self.delete(Self::name(key)).is_err() {
            self.mark_error()
        }
    }
    fn clear_all(&self) {
        if self.clear().is_err() {
            self.mark_error()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn cache_roundtrip_and_symlink_rejection() {
        let dir = std::env::temp_dir().join(format!(
            "protun-cache-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let c = DiskCache::open(&dir).unwrap();
        c.put(CacheKey::ApiSession, b"test session".to_vec());
        assert_eq!(c.get(CacheKey::ApiSession).unwrap(), b"test session");
        assert_eq!(
            fs::metadata(dir.join("session.bin")).unwrap().mode() & 0o777,
            0o600
        );
        fs::remove_file(dir.join("session.bin")).unwrap();
        std::os::unix::fs::symlink("private-key.bin", dir.join("session.bin")).unwrap();
        assert!(c.get(CacheKey::ApiSession).is_none());
        assert!(c.failed());
        fs::remove_file(dir.join("session.bin")).unwrap();
        drop(c);
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn session_fork_is_private_and_consumed_once() {
        let dir = std::env::temp_dir().join(format!(
            "protun-fork-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let c = DiskCache::open(&dir).unwrap();
        c.save_selector(&Selector {
            selector: "synthetic-test-selector".into(),
            cookies: vec![],
        })
        .unwrap();
        assert_eq!(
            fs::metadata(dir.join("fork.json")).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(
            c.take_selector().unwrap().unwrap().selector,
            "synthetic-test-selector"
        );
        assert!(c.take_selector().unwrap().is_none());
        drop(c);
        fs::remove_dir(dir).unwrap();
    }
}

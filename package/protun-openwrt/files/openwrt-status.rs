// SPDX-License-Identifier: GPL-3.0-or-later
// Fresh, atomic host status. Contains no keys, account tokens or certificate data.
use super::failure;
use protun::api::state::{ConnectionState, InterfaceState, Protocol, VpnState};
use serde_json::{Value, json};
use std::{ffi::CString, fs::{File, OpenOptions}, io::{self, Write},
    os::{fd::{AsRawFd, FromRawFd}, unix::fs::{MetadataExt, OpenOptionsExt}},
    path::Path, time::{Duration, Instant}};

pub struct Status {
    dir: Option<File>,
    name: String,
    instance: String,
    boot: String,
    state: Value,
    written: Instant,
    sequence: u64,
}
fn monotonic() -> io::Result<u64> {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ts.tv_sec as u64)
}
fn protocol(p: Protocol) -> &'static str {
    match p { Protocol::Stealth => "stealth", Protocol::WireguardUdp => "wireguard-udp", Protocol::WireguardTcp => "wireguard-tcp" }
}
impl Status {
    pub fn open(path: Option<&str>, instance: Option<&str>, probe: bool) -> io::Result<Self> {
        let mut s = Self { dir:None, name:String::new(), instance:instance.unwrap_or("").into(),
            boot:String::new(), state:json!({"state":"starting","connected":false,"tun_up":false,"ready":false}),
            written:Instant::now(), sequence:0 };
        if probe || path.is_none() { return Ok(s); }
        let path=Path::new(path.unwrap());
        let parent=path.parent().ok_or_else(||failure("Missing status directory"))?;
        s.name=path.file_name().and_then(|v|v.to_str()).ok_or_else(||failure("Invalid status file"))?.into();
        if !path.is_absolute() || s.name=="." || s.name==".." || !s.name.bytes().all(|b|b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || s.instance.is_empty() || s.instance.len()>64 || !s.instance.bytes().all(|b|b.is_ascii_alphanumeric() || b"_-".contains(&b)) {
            return Err(failure("Invalid status path or instance"));
        }
        let dir=OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC).open(parent)?;
        let m=dir.metadata()?;
        if m.uid()!=unsafe{libc::geteuid()} || m.mode() & 0o077 != 0 {
            return Err(failure("Status directory must be owned and private (0700)"));
        }
        s.boot=std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim().into();
        s.dir=Some(dir);s.publish()?;Ok(s)
    }
    pub fn update(&mut self, state:&VpnState) -> io::Result<()> {
        let tun_up=matches!(state.interface_state,InterfaceState::Up{error:None});
        self.state=match &state.connection_state {
            ConnectionState::Connected{peer,..} => json!({"state":"connected","connected":true,"tun_up":tun_up,"ready":tun_up,
                "protocol":protocol(peer.protocol),"peer":peer.peer_id,"port":peer.port}),
            ConnectionState::Connecting{..} => json!({"state":"connecting","connected":false,"tun_up":tun_up,"ready":false}),
            ConnectionState::ConnectingToLocalAgent{..} => json!({"state":"local-agent","connected":false,"tun_up":tun_up,"ready":false}),
            ConnectionState::Disconnected{..} => json!({"state":"disconnected","connected":false,"tun_up":tun_up,"ready":false}),
        };
        self.publish()
    }
    pub fn heartbeat(&mut self) -> io::Result<()> {
        if self.written.elapsed()>=Duration::from_secs(1) { self.publish()?; }
        Ok(())
    }
    pub fn stop(&mut self) -> io::Result<()> {
        self.state=json!({"state":"stopped","connected":false,"tun_up":false,"ready":false});self.publish()
    }
    fn publish(&mut self) -> io::Result<()> {
        let Some(dir)=&self.dir else {return Ok(())};
        self.state["version"]=json!(1);self.state["pid"]=json!(std::process::id());
        self.state["instance"]=json!(self.instance);self.state["boot_id"]=json!(self.boot);
        self.state["monotonic"]=json!(monotonic()?);
        let bytes=serde_json::to_vec(&self.state).map_err(|_|failure("Cannot encode status"))?;
        self.sequence+=1;
        let temporary=CString::new(format!(".status-{}-{}-{}",self.instance,std::process::id(),self.sequence)).unwrap();
        let name=CString::new(self.name.as_str()).unwrap();let fd=unsafe{libc::openat(dir.as_raw_fd(),temporary.as_ptr(),
            libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL|libc::O_NOFOLLOW|libc::O_CLOEXEC,0o600 as libc::mode_t)};
        if fd<0{return Err(io::Error::last_os_error())}
        let result=(||{let mut file=unsafe{File::from_raw_fd(fd)};file.write_all(&bytes)?;
            if unsafe{libc::renameat(dir.as_raw_fd(),temporary.as_ptr(),dir.as_raw_fd(),name.as_ptr())}!=0{return Err(io::Error::last_os_error())} Ok(())})();
        if result.is_err(){unsafe{libc::unlinkat(dir.as_raw_fd(),temporary.as_ptr(),0)};}
        self.written=Instant::now();result
    }
}
impl Drop for Status { fn drop(&mut self){let _=self.stop();} }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_status_location_is_rejected_and_probe_never_writes() {
        assert!(Status::open(Some("/tmp/test-state.json"),Some("test-instance"),false).is_err());
        assert!(Status::open(Some("/does-not-exist/state"),Some("test-instance"),true).is_ok());
        assert!(Status::open(Some("relative/state"),Some("test-instance"),false).is_err());
    }
}

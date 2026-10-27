// SPDX-License-Identifier: GPL-3.0-or-later
// Production Muon APIs; no test_utils feature or development login helper.
use super::{
    STOP,
    cache::{DiskCache, Selector, SelectorCookie},
    config::Config,
    failure, protect,
};
use async_compat::Compat;
use muon::{
    App, Environment,
    auth::LoginFlow,
    cookie_store::{CookieStore, CookieStoreSetResult},
    rt::{
        Monotonic, MuonSystemTime, OperatingSystem, Resolve, SinceUnixEpoch, SystemTimeFactory,
        TcpConnect,
    },
};
use pvpnclient::{cookie_store, url::Url};
use std::{
    ffi::CString,
    future::Future,
    io::{self, Write},
    os::fd::AsRawFd,
    pin::Pin,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
struct Clock(Instant);
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
struct Dialer {
    device: CString,
    mark: Option<u32>,
}
impl TcpConnect for Dialer {
    type Socket = Compat<tokio::net::TcpStream>;
    type Err = io::Error;
    async fn tcp_connect(&self, addr: std::net::SocketAddr) -> io::Result<Self::Socket> {
        let socket = if addr.is_ipv4() {
            tokio::net::TcpSocket::new_v4()?
        } else {
            tokio::net::TcpSocket::new_v6()?
        };
        if !protect(socket.as_raw_fd(), &self.device, self.mark) {
            return Err(failure("Cannot protect authentication socket"));
        }
        let stream = tokio::time::timeout(Duration::from_secs(30), socket.connect(addr))
            .await
            .map_err(|_| failure("Authentication transport timed out"))??;
        Ok(Compat::new(stream))
    }
}
#[derive(Clone, Debug, Default)]
struct Resolver;
impl Resolve for Resolver {
    type Err = io::Error;
    async fn resolve(&self, host: &str) -> io::Result<Vec<std::net::IpAddr>> {
        Ok(tokio::net::lookup_host((host, 443))
            .await?
            .map(|a| a.ip())
            .collect())
    }
}
#[derive(Clone, Debug)]
struct Os {
    clock: Clock,
    dialer: Dialer,
    resolver: Resolver,
}
impl OperatingSystem for Os {
    type Resolver = Resolver;
    type TcpConnector = Dialer;
    type Time = Clock;
    fn get_time_capabilities(&self) -> &Clock {
        &self.clock
    }
    fn get_tcp_connector(&self) -> &Dialer {
        &self.dialer
    }
    fn get_resolver(&self) -> &Resolver {
        &self.resolver
    }
}
#[derive(Clone, Debug)]
struct Executor;
impl muon::rt::Spawn for Executor {
    fn spawn_obj(
        &self,
        future: muon::rt::FutureObj<'static, ()>,
    ) -> Result<(), muon::rt::SpawnError> {
        tokio::spawn(future);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct Jar(Arc<Mutex<cookie_store::CookieStore>>);
impl std::fmt::Debug for Jar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateCookieJar")
    }
}
impl CookieStore for Jar {
    fn set(&mut self, cookie: cookie_store::Cookie, url: &Url) -> CookieStoreSetResult {
        let _ = self.0.lock().unwrap().insert(cookie.into_owned(), url);
        CookieStoreSetResult::Pass
    }
    fn get(&self, url: &Url) -> Vec<cookie_store::RawCookie<'_>> {
        self.0
            .lock()
            .unwrap()
            .get_request_values(url)
            .map(|(name, value)| cookie_store::RawCookie::new(name.to_owned(), value.to_owned()))
            .collect()
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
fn prompt(label: &str, secret: bool) -> io::Result<String> {
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
fn erase(value: &mut String) {
    // Overwriting with zero bytes preserves UTF-8 validity. The SDK may have its
    // own copies; this only erases the command's owned prompt buffer.
    unsafe { value.as_mut_vec().fill(0) };
}

pub fn login(config: &Config, cache: &DiskCache) -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(failure("Run account login as root"));
    }
    let agent = config
        .local_agent
        .as_ref()
        .ok_or_else(|| failure("Configure local_agent before login"))?;
    let username = prompt("Proton username: ", false)?;
    let mut password = prompt("Proton password: ", true)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result=runtime.block_on(async {
        let app=App::new(&agent.login_app_version).map_err(|_|failure("Invalid login app version"))?
            .with_user_agent(concat!("protun-openwrt/",env!("CARGO_PKG_VERSION")));
        let jar=Jar::default();
        let session=muon::Client::builder(app,Environment::new_prod())
            .with_operating_system(Os { clock:Clock::default(), dialer:Dialer {
                device:CString::new(config.uplink_device.as_str()).map_err(|_|failure("Invalid uplink"))?,
                mark:config.socket_mark }, resolver:Resolver },rand::rng())
            .with_multi_thread_executor(Executor).without_persistence::<()>()
            .with_cookie_store(jar.clone()).build().map_err(|_|failure("Cannot initialize Proton authentication"))?
            .new_session_without_credentials(()).await.map_err(|_|failure("Cannot initialize Proton session"))?;
        let flow=tokio::time::timeout(Duration::from_secs(120),session.auth().login(username.trim(),&password)).await;
        erase(&mut password);
        let session=match flow.map_err(|_|failure("Proton login timed out"))? {
            LoginFlow::Ok(session,_)=>session,
            LoginFlow::TwoFactor(flow,_)=>{
                if !flow.has_totp(){return Err(failure("This account requires a hardware security key; use --import-session from an authenticated client"))}
                let mut code=prompt("Proton authenticator code: ",true)?;
                let result=tokio::time::timeout(Duration::from_secs(60),flow.totp(&code)).await;
                erase(&mut code);
                result.map_err(|_|failure("Two-factor login timed out"))?
                    .map_err(|_|failure("Two-factor login failed"))?
            },
            LoginFlow::Failed {..}=>return Err(failure("Proton login failed; verify credentials, app version and account verification requirements")),
        };
        let child=App::new(&agent.app_version).map_err(|_|failure("Invalid VPN app version"))?;
        let selector=tokio::time::timeout(Duration::from_secs(60),pvpnclient::util::fork_muon_session(session,child))
            .await.map_err(|_|failure("Session fork timed out"))?.map_err(|_|failure("Proton session fork failed"))?;
        // Only forward unexpired cookies scoped to the VPN API. Do not copy
        // cookies belonging to a different parent application's host.
        let vpn_api=Url::parse("https://vpn-api.proton.me/").map_err(|_|failure("Invalid VPN API URL"))?;
        let cookies=jar.0.lock().unwrap().get_request_values(&vpn_api)
            .map(|(name,value)|SelectorCookie{name:name.to_owned(),value:value.to_owned()}).collect();
        cache.save_selector(&Selector{selector,cookies})
    });
    erase(&mut password);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookies_are_scoped_and_updates_replace_old_values() {
        let vpn = Url::parse("https://vpn-api.proton.me/").unwrap();
        let other = Url::parse("https://account.proton.me/").unwrap();
        let mut jar = Jar::default();
        for value in ["first", "second"] {
            let cookie =
                cookie_store::Cookie::parse(format!("test_cookie={value}; Path=/; Secure"), &vpn)
                    .unwrap();
            jar.set(cookie, &vpn);
        }
        let cookies = jar.get(&vpn);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value(), "second");
        assert!(jar.get(&other).is_empty());
    }
}

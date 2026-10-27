// SPDX-License-Identifier: GPL-3.0-or-later
// Protun 3.0.5 public API; OpenWrt retains ownership of network policy.
use base64::{Engine, engine::general_purpose::STANDARD};
use protun::api::connection::{CacheKey, Connection, PersistentCache, WgClientPrivateKey};
use protun::api::events::{ErrorEvent, Event};
use protun::api::state::{ConnectionState, InterfaceState, VpnState};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[path = "openwrt-auth.rs"]
mod auth;
#[path = "openwrt-cache.rs"]
mod cache;
#[path = "openwrt-config.rs"]
mod config;
use cache::{DiskCache, Selector};
use config::Config;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

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

fn interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < libc::IFNAMSIZ
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}

fn decode_key(value: &str) -> io::Result<[u8; 32]> {
    let bytes: [u8; 32] = STANDARD
        .decode(value.trim())
        .map_err(|_| failure("Invalid base64 key"))?
        .try_into()
        .map_err(|_| failure("Key must decode to exactly 32 bytes"))?;
    if bytes == [0; 32] {
        return Err(failure("Zero keys are not valid credentials"));
    }
    Ok(bytes)
}

#[derive(Default)]
struct MemoryCache(Mutex<HashMap<CacheKey, Vec<u8>>>);
impl PersistentCache for MemoryCache {
    fn put(&self, key: CacheKey, bytes: Vec<u8>) {
        self.0.lock().unwrap().insert(key, bytes);
    }
    fn get(&self, key: CacheKey) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(&key).cloned()
    }
    fn remove(&self, key: CacheKey) {
        self.0.lock().unwrap().remove(&key);
    }
    fn clear_all(&self) {
        self.0.lock().unwrap().clear();
    }
}

fn device_index(device: &CString) -> u32 {
    unsafe { libc::if_nametoindex(device.as_ptr()) }
}

fn create_tun(name: &str) -> io::Result<OwnedFd> {
    let name_c = CString::new(name).map_err(|_| failure("Invalid TUN name"))?;
    if device_index(&name_c) != 0 {
        return Err(failure(
            "TUN name already exists; refusing to attach to an existing device",
        ));
    }
    let fd: OwnedFd = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open("/dev/net/tun")?
        .into();
    // libc's target-specific ifreq layout avoids hard-coded ioctl buffer sizes.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (dest, src) in request.ifr_name.iter_mut().zip(name.bytes()) {
        *dest = src as libc::c_char;
    }
    request.ifr_ifru.ifru_flags =
        (libc::IFF_TUN | libc::IFF_NO_PI | libc::IFF_TUN_EXCL) as libc::c_short;
    if unsafe { libc::ioctl(fd.as_raw_fd(), libc::TUNSETIFF, &request) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // The descriptor is not persistent: shutdown removes only this new device.
    // netifd configures its addresses, link state and MTU when it appears.
    Ok(fd)
}

fn protect(fd: i32, uplink: &CString, mark: Option<u32>) -> bool {
    let bytes = uplink.as_bytes_with_nul();
    if unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            bytes.as_ptr().cast(),
            bytes.len() as libc::socklen_t,
        )
    } != 0
    {
        eprintln!("protun: failed to bind transport socket to uplink");
        return false;
    }
    if let Some(mark) = mark {
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_MARK,
                (&mark as *const u32).cast(),
                std::mem::size_of_val(&mark) as libc::socklen_t,
            )
        } != 0
        {
            eprintln!("protun: failed to apply configured transport mark");
            return false;
        }
    }
    true
}

fn run(config: Config, private_key: Option<WgClientPrivateKey>, probe: bool) -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(failure("Run the client or probe as root"));
    }
    let uplink =
        CString::new(config.uplink_device.as_str()).map_err(|_| failure("Invalid uplink"))?;
    if device_index(&uplink) == 0 {
        return Err(failure("Configured physical uplink does not exist"));
    }
    let tun = if probe {
        None
    } else {
        Some(create_tun(&config.tun_name)?)
    };
    // Coalesce wakeups while retaining the latest state, including disconnects.
    // Callbacks never block on a full queue or build an unbounded backlog.
    let (sender, receiver) = mpsc::sync_channel(1);
    let latest_state = Arc::new(Mutex::new(None));
    let callback_state = Arc::clone(&latest_state);
    let mark = config.socket_mark;
    let disk_cache = config
        .local_agent
        .as_ref()
        .map(|a| DiskCache::open(Path::new(&a.cache_dir)))
        .transpose()?;
    let events = Arc::new(AgentEvents::default());
    let callback_events = Arc::clone(&events);
    let cache: Box<dyn PersistentCache> = match &disk_cache {
        Some(cache) => Box::new(cache.clone()),
        None => Box::<MemoryCache>::default(),
    };
    let connection = Connection::unix_connect(
        config.connection(private_key)?,
        tun.map(IntoRawFd::into_raw_fd),
        Box::new(move |state: VpnState| {
            *callback_state.lock().unwrap() = Some(state);
            let _ = sender.try_send(());
        }),
        Box::new(move |event: Event| {
            if let Event::LocalAgentError { error } = event {
                match error {
                    ErrorEvent::ForkSelectorNeeded => {
                        callback_events.need_login.store(true, Ordering::Relaxed)
                    }
                    ErrorEvent::ApiError {
                        refresh_token_invalid: true,
                        ..
                    } => callback_events.need_login.store(true, Ordering::Relaxed),
                    ErrorEvent::CertificateRefreshFatalError => {
                        callback_events.fatal.store(true, Ordering::Relaxed)
                    }
                    ErrorEvent::LocalAgentSettingPolicyRefused { .. } => callback_events
                        .policy_refused
                        .store(true, Ordering::Relaxed),
                    ErrorEvent::ApiError { .. } => {
                        callback_events.api_failed.store(true, Ordering::Relaxed)
                    }
                }
            }
        }),
        Some(Box::new(move |fd| protect(fd, &uplink, mark))),
        cache,
    );
    let deadline = Instant::now() + Duration::from_secs(config.connect_timeout_seconds);
    let mut connected_once = false;
    let result = loop {
        if disk_cache.as_ref().is_some_and(DiskCache::failed) {
            break Err(failure(
                "Cannot safely read or persist Proton session cache",
            ));
        }
        if events.fatal.swap(false, Ordering::Relaxed) {
            break Err(failure("Proton certificate renewal failed; log in again"));
        }
        if events.need_login.swap(false, Ordering::Relaxed) {
            let selector = disk_cache
                .as_ref()
                .ok_or_else(|| failure("Missing Local Agent cache"))
                .and_then(DiskCache::take_selector);
            match selector {
                Ok(Some(selector)) => connection.provide_api_fork_selector(selector),
                Ok(None) => {
                    break Err(failure(
                        "Proton login required; stop the service and run --login or --import-session",
                    ));
                }
                Err(error) => break Err(error),
            }
        }
        if events.policy_refused.swap(false, Ordering::Relaxed) {
            eprintln!(
                "protun: server refused a requested Local Agent setting; check account/server support"
            );
        }
        if events.api_failed.swap(false, Ordering::Relaxed) {
            eprintln!("protun: Proton API request failed; the engine may retry");
        }
        if STOP.load(Ordering::Relaxed) {
            break if probe {
                Err(failure("Probe interrupted before success"))
            } else {
                Ok(())
            };
        }
        if !connected_once && Instant::now() >= deadline {
            break Err(failure(
                "VPN connection timed out; credentials and endpoint require live verification",
            ));
        }
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(()) => {
                let Some(state) = latest_state.lock().unwrap().take() else {
                    continue;
                };
                if matches!(
                    state.interface_state,
                    InterfaceState::Up { error: Some(_) }
                        | InterfaceState::Down {
                            last_error: Some(_)
                        }
                ) {
                    break Err(failure("TUN I/O failed; restarting requires a new device"));
                }
                match state.connection_state {
                    ConnectionState::Connected { peer, agent_info } => {
                        if !config.protocol.allows(peer.protocol) {
                            break Err(failure(
                                "Connected protocol does not match the configured selection",
                            ));
                        }
                        connected_once = true;
                        eprintln!(
                            "protun: connected protocol={:?} peer={:?} port={}",
                            peer.protocol, peer.peer_id, peer.port
                        );
                        if let Some(info) = agent_info {
                            eprintln!("protun: local-agent applied settings={:?}", info.settings);
                        }
                        if probe {
                            break Ok(());
                        }
                    }
                    ConnectionState::Disconnected { .. } => {
                        break Err(failure("Protun disconnected"));
                    }
                    ConnectionState::Connecting { .. } => {
                        eprintln!("protun: connecting protocol={:?}", config.protocol)
                    }
                    _ => {}
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(failure("Protun state channel closed"));
            }
        }
    };
    connection.disconnect_and_wait();
    result
}

#[derive(Default)]
struct AgentEvents {
    need_login: AtomicBool,
    fatal: AtomicBool,
    policy_refused: AtomicBool,
    api_failed: AtomicBool,
}

fn install_signals() -> io::Result<()> {
    unsafe {
        let limits = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::setrlimit(libc::RLIMIT_CORE, &limits) < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = stop as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        // No SA_RESTART: a signal must interrupt hidden terminal input so its
        // guard can restore echo before exiting.
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGTSTP] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--help" {
        println!(
            "protun-openwrt --check-config|--probe|--config /absolute/path/client.json\nProtocols: stealth (default), wireguard-udp, wireguard-tcp, auto.\nAccount commands: --login CONFIG, --import-session CONFIG FORK_JSON, --clear-session CONFIG.\n--capabilities lists supported interfaces. OpenWrt retains network policy. --probe creates no TUN device."
        );
        return;
    }
    if args.len() == 2 && args[1] == "--capabilities" {
        println!(
            "{}",
            serde_json::json!({
                "protocols":["stealth","wireguard-udp","wireguard-tcp","auto"],
                "default_protocol":"stealth", "multiple_peers":true,
                "authentication":["private-key","password-totp","session-fork-import"],
                "local_agent_settings":["netshield_level","split_tcp","soft_jail","port_forwarding","random_nat","circumvention_routing"]
            })
        );
        return;
    }
    let result = (|| {
        let commands = [
            "--check-config",
            "--probe",
            "--config",
            "--login",
            "--import-session",
            "--clear-session",
        ];
        if args.len() < 3
            || !commands.contains(&args[1].as_str())
            || args.len() != (if args[1] == "--import-session" { 4 } else { 3 })
            || !Path::new(&args[2]).is_absolute()
        {
            return Err(failure(
                "Use --help for usage; an absolute configuration path is required",
            ));
        }
        let (config, key) = Config::load(Path::new(&args[2]))?;
        if args[1] == "--check-config" {
            println!(
                "Configuration valid; {}; connectivity has not been tested",
                config.summary()?
            );
            return Ok(());
        }
        install_signals()?;
        if ["--login", "--import-session", "--clear-session"].contains(&args[1].as_str()) {
            if unsafe { libc::geteuid() } != 0 {
                return Err(failure("Run session management as root"));
            }
            let agent = config
                .local_agent
                .as_ref()
                .ok_or_else(|| failure("Configure local_agent for account/session commands"))?;
            let cache = DiskCache::open(Path::new(&agent.cache_dir))?;
            match args[1].as_str() {
                "--login" => {
                    auth::login(&config, &cache)?;
                    println!(
                        "Login fork saved privately; run --probe to verify VPN and Local Agent"
                    );
                }
                "--clear-session" => {
                    cache.clear()?;
                    println!(
                        "Local session cache cleared; server-side revocation is managed in your Proton account"
                    );
                }
                "--import-session" => {
                    if !Path::new(&args[3]).is_absolute() {
                        return Err(failure("An absolute fork file path is required"));
                    }
                    let selector: Selector =
                        serde_json::from_slice(&read_file(Path::new(&args[3]), true)?)
                            .map_err(|_| failure("Invalid session fork JSON"))?;
                    if selector.selector.is_empty() {
                        return Err(failure("Empty session fork selector"));
                    }
                    cache.save_selector(&selector)?;
                    println!("Session fork saved privately; run --probe before migrating traffic");
                }
                _ => unreachable!(),
            }
            return Ok(());
        }
        run(config, key, args[1] == "--probe")
    })();
    if let Err(error) = result {
        eprintln!("protun: {error}");
        std::process::exit(1);
    }
}

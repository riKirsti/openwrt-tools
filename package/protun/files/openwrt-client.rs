// SPDX-License-Identifier: GPL-3.0-or-later
// Protun 3.0.5 public API; OpenWrt retains ownership of network policy.
use base64::{Engine, engine::general_purpose::STANDARD};
use protun::api::connection::{
    CacheKey, Connection, ConnectionMode, InitialConnectionConfig, IpAddress, PeerInfo,
    PersistentCache, SniStrategy, WgClientPrivateKey, WgPeerPublicKey,
};
use protun::api::events::Event;
use protun::api::state::{ConnectionState, InterfaceState, Protocol, VpnState};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::net::IpAddr;
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    tun_name: String,
    uplink_device: String,
    server_ip: IpAddr,
    server_public_key: String,
    private_key_file: String,
    tls_port: u16,
    connect_timeout_seconds: u64,
    #[serde(default)]
    socket_mark: Option<u32>,
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

impl Config {
    fn load(path: &Path) -> io::Result<(Self, WgClientPrivateKey)> {
        let config: Self = serde_json::from_slice(&read_file(path, false)?)
            .map_err(|_| failure("Invalid client JSON or unknown configuration field"))?;
        if !interface_name(&config.tun_name)
            || !interface_name(&config.uplink_device)
            || config.tun_name == config.uplink_device
            || config.uplink_device == "lo"
            || config.tls_port == 0
            || !(1..=600).contains(&config.connect_timeout_seconds)
            || config.server_ip.is_unspecified()
            || config.server_ip.is_multicast()
            || config.server_ip.is_loopback()
            || !Path::new(&config.private_key_file).is_absolute()
        {
            return Err(failure("Invalid interface, endpoint, timeout or key path"));
        }
        decode_key(&config.server_public_key)?;
        let key_bytes = read_file(Path::new(&config.private_key_file), true)?;
        let key_text =
            std::str::from_utf8(&key_bytes).map_err(|_| failure("Invalid key file encoding"))?;
        let key = WgClientPrivateKey(decode_key(key_text)?);
        Ok((config, key))
    }

    fn connection(&self, private_key: WgClientPrivateKey) -> InitialConnectionConfig {
        InitialConnectionConfig {
            peers: vec![PeerInfo {
                peer_id: "proton".into(),
                server_ip: IpAddress(self.server_ip),
                server_public_key: WgPeerPublicKey(
                    decode_key(&self.server_public_key).expect("validated key"),
                ),
                udp_ports: vec![],
                tcp_ports: vec![],
                tls_ports: vec![self.tls_port],
                priority: 0,
                exit_label: None,
                server_name: None,
            }],
            network_available: true,
            pcap_file: None,
            connection_mode: ConnectionMode::NoLocalAgent {
                wg_private_key: Some(private_key),
            },
            sni_strategy: SniStrategy::Random,
        }
    }
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

fn run(config: Config, private_key: WgClientPrivateKey, probe: bool) -> io::Result<()> {
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
    // signal() receives only a function pointer; the handler only sets an atomic.
    unsafe {
        if libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t) == libc::SIG_ERR
            || libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t) == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
    }
    let connection = Connection::unix_connect(
        config.connection(private_key),
        tun.map(IntoRawFd::into_raw_fd),
        Box::new(move |state: VpnState| {
            *callback_state.lock().unwrap() = Some(state);
            let _ = sender.try_send(());
        }),
        Box::new(|_: Event| {}),
        Some(Box::new(move |fd| protect(fd, &uplink, mark))),
        Box::<MemoryCache>::default(),
    );
    let deadline = Instant::now() + Duration::from_secs(config.connect_timeout_seconds);
    let mut connected_once = false;
    let result = loop {
        if STOP.load(Ordering::Relaxed) {
            break if probe {
                Err(failure("Probe interrupted before success"))
            } else {
                Ok(())
            };
        }
        if !connected_once && Instant::now() >= deadline {
            break Err(failure(
                "Stealth handshake timed out; credentials and endpoint require live verification",
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
                    ConnectionState::Connected { peer, .. } => {
                        if peer.protocol != Protocol::Stealth {
                            break Err(failure("Refusing a non-Stealth connection"));
                        }
                        connected_once = true;
                        eprintln!("protun: connected protocol=Stealth port={}", peer.port);
                        if probe {
                            break Ok(());
                        }
                    }
                    ConnectionState::Disconnected { .. } => {
                        break Err(failure("Protun disconnected"));
                    }
                    ConnectionState::Connecting { .. } => {
                        eprintln!("protun: connecting protocol=Stealth")
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--help" {
        println!(
            "protun-openwrt --check-config|--probe|--config /absolute/path/client.json\nStealth only. Network policy remains managed by OpenWrt. --probe performs a handshake without creating a TUN device."
        );
        return;
    }
    let result = (|| {
        if args.len() != 3
            || !["--check-config", "--probe", "--config"].contains(&args[1].as_str())
            || !Path::new(&args[2]).is_absolute()
        {
            return Err(failure(
                "Use --help for usage; an absolute configuration path is required",
            ));
        }
        let (config, key) = Config::load(Path::new(&args[2]))?;
        if args[1] == "--check-config" {
            println!("Configuration valid; network connectivity has not been tested");
            return Ok(());
        }
        run(config, key, args[1] == "--probe")
    })();
    if let Err(error) = result {
        eprintln!("protun: {error}");
        std::process::exit(1);
    }
}

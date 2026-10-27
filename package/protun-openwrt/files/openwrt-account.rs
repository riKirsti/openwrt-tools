// SPDX-License-Identifier: GPL-3.0-or-later
// Compatibility account/API entry points. No routing or tunnel activation here.
use super::{auth, failure};
#[path = "openwrt-account-io.rs"]
mod account_io;
use base64::{Engine, engine::general_purpose::STANDARD};
use muon::{
    App, Environment, ProtonRequest, Session, SessionCredentials,
    auth::{Auth, LoginFlow},
    common::{Context, RetryPolicy, sdk::Sdk},
    store::Store,
};
use serde_json::{Value, json};
#[cfg(not(test))]
use std::os::unix::net::UnixStream;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
static TEMP: AtomicU64 = AtomicU64::new(0);
#[derive(Clone)]
struct LoopbackEnvironment(muon::common::Server);
impl muon::env::Env for LoopbackEnvironment {
    fn servers(&self, _: &muon::app::AppVersion) -> Vec<muon::common::Server> {
        vec![self.0.clone()]
    }
    fn ar_pins(&self) -> Option<&muon::tls::pins::TlsPinSet> {
        None
    }
    fn api_pins(&self) -> Option<&muon::tls::pins::TlsPinSet> {
        None
    }
}
fn environment(raw: &str, isolated: bool) -> io::Result<Environment> {
    if raw == "https://vpn-api.proton.me" {
        return Ok(Environment::new_prod());
    }
    let url = pvpnclient::url::Url::parse(raw).map_err(|_| failure("Invalid API URL"))?;
    let numeric = url
        .host_str()
        .map(|h| h.trim_matches(['[', ']']))
        .and_then(|h| h.parse::<IpAddr>().ok());
    if !isolated
        || !numeric.is_some_and(|ip| ip.is_loopback())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !matches!(url.scheme(), "http" | "https")
    {
        return Err(failure(
            "Only the official API or an explicitly isolated numeric loopback fixture is accepted",
        ));
    }
    Ok(Environment::new_custom(LoopbackEnvironment(
        raw.parse()
            .map_err(|_| failure("Invalid loopback API endpoint"))?,
    )))
}
fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
fn timestamp(at: i64) -> io::Result<String> {
    OffsetDateTime::from_unix_timestamp(at)
        .map_err(|_| failure("Invalid timestamp"))?
        .format(&Rfc3339)
        .map_err(|_| failure("Cannot format timestamp"))
}
fn private_read(path: &Path) -> io::Result<Vec<u8>> {
    super::read_file(path, true)
}
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(failure("Refusing unsafe destination"));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = parent.join(format!(
        ".proton-account-{}-{}",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    let _ = fs::remove_file(temp);
    result
}

#[derive(Clone)]
struct LegacyStore {
    path: Option<PathBuf>,
    value: Arc<Mutex<Value>>,
    auth: Arc<Mutex<Option<Auth>>>,
    failed: Arc<AtomicBool>,
}
impl std::fmt::Debug for LegacyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateAccountStore")
    }
}
impl LegacyStore {
    fn new(path: Option<PathBuf>, saved: Value) -> io::Result<Self> {
        let initial = if saved.get("session").is_some() {
            let credentials: SessionCredentials = serde_json::from_value(saved["session"].clone())
                .map_err(|_| failure("Invalid saved authorization"))?;
            if credentials.uid().is_empty()
                || credentials.access_token().is_empty()
                || credentials.refresh_token().is_empty()
            {
                return Err(failure("Incomplete saved authorization"));
            }
            Some(credentials.as_auth())
        } else {
            None
        };
        Ok(Self {
            path,
            value: Arc::new(Mutex::new(saved)),
            auth: Arc::new(Mutex::new(initial)),
            failed: Arc::new(AtomicBool::new(false)),
        })
    }
    fn check(&self) -> io::Result<()> {
        if self.failed.load(Ordering::Relaxed) {
            Err(failure(
                "Authorization could not be saved; account operation stopped",
            ))
        } else {
            Ok(())
        }
    }
    fn persist(&self, auth: &Auth) -> io::Result<()> {
        let credentials = SessionCredentials::try_from(auth.clone())
            .map_err(|_| failure("Missing account credentials"))?;
        let encoded = serde_json::to_value(credentials)
            .map_err(|_| failure("Cannot encode authorization"))?;
        let mut saved = self
            .value
            .lock()
            .map_err(|_| failure("Account store unavailable"))?;
        let same = ["UID", "AccessToken", "RefreshToken", "Scopes"]
            .iter()
            .all(|k| saved["session"][*k] == encoded[*k]);
        if same {
            return Ok(());
        }
        // Anonymous pre-login credentials must never overwrite an authorized account.
        let scopes = encoded["Scopes"].as_array().cloned().unwrap_or_default();
        if !scopes.iter().any(|v| v == "vpn" || v == "full") {
            return Ok(());
        }
        let mut next = saved.clone();
        next["session"] = encoded;
        // Muon does not expose token expiry. Mark it unknown rather than invent it;
        // a legacy client rolling back will refresh and learn the actual expiry.
        next["session"]["ExpiresIn"] = json!(0);
        next["session"]["Code"] = json!(1000);
        next["saved_at"] = json!(timestamp(now())?);
        next["expires_at"] = json!(timestamp(now())?);
        if let Some(path) = &self.path {
            atomic_write(
                path,
                &serde_json::to_vec(&next).map_err(|_| failure("Cannot encode authorization"))?,
            )?
        }
        *saved = next;
        Ok(())
    }
}
impl Store for LegacyStore {
    type Key = ();
    fn set_auth(&mut self, _: (), value: Auth) {
        *self.auth.lock().unwrap() = Some(value.clone());
        if self.persist(&value).is_err() {
            self.failed.store(true, Ordering::Relaxed)
        }
    }
    // Muon may discard rejected in-memory tokens; only confirmed explicit logout
    // or explicit --forget may remove the user's persistent authorization file.
    fn remove_auth(&mut self, _: &()) {
        *self.auth.lock().unwrap() = None;
    }
    fn get_all_auth(&self) -> HashMap<(), Auth> {
        self.auth
            .lock()
            .unwrap()
            .clone()
            .map(|a| HashMap::from([((), a)]))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
struct Options {
    values: HashMap<String, String>,
    explicit: HashSet<String>,
}
impl Options {
    fn text(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }
    fn flag(&self, key: &str) -> bool {
        self.text(key) == "true"
    }
    fn parse(args: &[String]) -> io::Result<Self> {
        let strings = [
            "api-url",
            "username",
            "password",
            "server",
            "output",
            "revoke-serial",
            "duration",
            "countries",
            "fixture-totp",
            "hv-token",
            "dns",
            "allowed-ips",
            "device-name",
            "session-duration",
            "renew-serial",
        ];
        let bools = [
            "list-servers",
            "json",
            "metadata-json",
            "free-only",
            "ipv6",
            "no-save",
            "non-interactive",
            "password-stdin",
            "force-refresh",
            "logout",
            "forget",
            "clear-session",
            "no-session",
            "p2p-only",
            "secure-core",
            "accelerator",
            "port-forwarding",
            "moderate-nat",
            "debug",
            "list-configs",
            "with-sessions",
        ];
        let mut o = Self {
            values: HashMap::from([
                ("api-url".into(), "https://vpn-api.proton.me".into()),
                ("output".into(), "protonvpn.conf".into()),
                ("duration".into(), "365d".into()),
                ("session-duration".into(), "0".into()),
                ("netshield".into(), "0".into()),
                ("p2p-only".into(), "true".into()),
                ("accelerator".into(), "true".into()),
            ]),
            explicit: HashSet::new(),
        };
        if let Ok(hv) = std::env::var("PROTONVPN_HV_TOKEN") {
            o.values.insert("hv-token".into(), hv);
        }
        let mut i = 0;
        while i < args.len() {
            let raw = args[i]
                .strip_prefix("--")
                .or_else(|| args[i].strip_prefix('-'))
                .ok_or_else(|| failure("Unexpected positional argument"))?;
            let (key, value) = raw
                .split_once('=')
                .map(|(k, v)| (k, Some(v)))
                .unwrap_or((raw, None));
            let value = if bools.contains(&key) {
                value.unwrap_or("true").to_owned()
            } else if strings.contains(&key) || key == "netshield" {
                if let Some(v) = value {
                    v.to_owned()
                } else {
                    i += 1;
                    args.get(i)
                        .ok_or_else(|| failure("Flag requires a value"))?
                        .clone()
                }
            } else {
                return Err(failure("Unknown account option"));
            };
            if bools.contains(&key) && value != "true" && value != "false" {
                return Err(failure("Invalid boolean option"));
            }
            o.explicit.insert(key.into());
            o.values.insert(key.into(), value);
            i += 1;
        }
        for (key, upper) in [("countries", true), ("dns", false), ("allowed-ips", false)] {
            let value = o
                .text(key)
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(|x| if upper { x.to_uppercase() } else { x.into() })
                .collect::<Vec<_>>()
                .join(",");
            o.values.insert(key.into(), value);
        }
        if o.flag("no-save") && !o.explicit.contains("duration") {
            o.values.insert("duration".into(), "7d".into());
        }
        o.validate()?;
        Ok(o)
    }
    fn validate(&self) -> io::Result<()> {
        environment(
            self.text("api-url"),
            std::env::var_os("PROTON_SESSION_DIR").is_some(),
        )?;
        if self.flag("non-interactive")
            && (self.flag("password-stdin")
                || !self.text("password").is_empty()
                || self.flag("clear-session")
                || self.flag("no-session"))
        {
            return Err(failure("Non-interactive authorization options conflict"));
        }
        if self.flag("password-stdin")
            && (!self.text("password").is_empty() || self.text("username").is_empty())
        {
            return Err(failure("Password input options conflict"));
        }
        if self.flag("port-forwarding") && self.flag("moderate-nat") {
            return Err(failure("Port forwarding conflicts with moderate NAT"));
        }
        let shield = self
            .text("netshield")
            .parse::<u8>()
            .map_err(|_| failure("Invalid NetShield level"))?;
        if shield > 2 {
            return Err(failure("Invalid NetShield level"));
        }
        if !self.text("countries").is_empty()
            && self
                .text("countries")
                .split(',')
                .any(|s| s.len() != 2 || !s.bytes().all(|c| c.is_ascii_uppercase()))
        {
            return Err(failure("Invalid country"));
        }
        for ip in self.text("dns").split(',').filter(|x| !x.is_empty()) {
            if ip.parse::<IpAddr>().is_err() {
                return Err(failure("Invalid DNS address"));
            }
        }
        for prefix in self
            .text("allowed-ips")
            .split(',')
            .filter(|x| !x.is_empty())
        {
            let (ip, bits) = prefix
                .split_once('/')
                .ok_or_else(|| failure("Invalid allowed prefix"))?;
            let ip = ip
                .parse::<IpAddr>()
                .map_err(|_| failure("Invalid allowed prefix"))?;
            let bits = bits
                .parse::<u8>()
                .map_err(|_| failure("Invalid allowed prefix"))?;
            if bits > if ip.is_ipv4() { 32 } else { 128 } {
                return Err(failure("Invalid allowed prefix"));
            }
        }
        let duration = minutes(self.text("duration"))?;
        if self.flag("no-save") && duration > 10080 {
            return Err(failure("Temporary certificate duration exceeds seven days"));
        }
        if self.text("session-duration") != "0" {
            duration_nanos(self.text("session-duration"))?;
        }
        Ok(())
    }
}
fn duration_nanos(raw: &str) -> io::Result<u64> {
    if let Some(days) = raw.strip_suffix('d') {
        let days = days
            .parse::<u64>()
            .map_err(|_| failure("Invalid day duration"))?;
        if days == 0 || days > 365 {
            return Err(failure("Invalid day duration"));
        }
        return Ok(days * 86400 * 1_000_000_000);
    }
    let mut remaining = raw.strip_prefix('+').unwrap_or(raw);
    let mut total = 0u128;
    while !remaining.is_empty() {
        let n = remaining
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .ok_or_else(|| failure("Duration unit required"))?;
        if n == 0 {
            return Err(failure("Invalid duration"));
        }
        let digits = &remaining[..n];
        remaining = &remaining[n..];
        let (unit, multiplier) = [
            ("ns", 1u128),
            ("us", 1000),
            ("µs", 1000),
            ("μs", 1000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find(|(u, _)| remaining.starts_with(u))
        .ok_or_else(|| failure("Invalid duration unit"))?;
        remaining = &remaining[unit.len()..];
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        if (whole.is_empty() && fraction.is_empty())
            || fraction.contains('.')
            || fraction.len() > 18
        {
            return Err(failure("Invalid duration number"));
        }
        let whole = if whole.is_empty() {
            0
        } else {
            whole
                .parse::<u128>()
                .map_err(|_| failure("Invalid duration number"))?
        };
        let part = if fraction.is_empty() {
            0
        } else {
            fraction
                .parse::<u128>()
                .map_err(|_| failure("Invalid duration fraction"))?
                .checked_mul(multiplier)
                .ok_or_else(|| failure("Duration overflow"))?
                / 10u128.pow(fraction.len() as u32)
        };
        total = total
            .checked_add(
                whole
                    .checked_mul(multiplier)
                    .and_then(|n| n.checked_add(part))
                    .ok_or_else(|| failure("Duration overflow"))?,
            )
            .ok_or_else(|| failure("Duration overflow"))?;
        if total > i64::MAX as u128 {
            return Err(failure("Duration overflow"));
        }
    }
    if total == 0 {
        return Err(failure("Invalid duration"));
    }
    Ok(total as u64)
}
fn minutes(raw: &str) -> io::Result<u64> {
    let n = duration_nanos(raw)?;
    if n < 600_000_000_000 || n > 365 * 86400 * 1_000_000_000 || n % 60_000_000_000 != 0 {
        return Err(failure("Invalid certificate duration"));
    }
    Ok(n / 60_000_000_000)
}
fn api_error(error: muon::Error) -> io::Error {
    if matches!(error.kind(), muon::ErrorKind::Auth)
        || error.http_code().is_some_and(|status| status == 401)
    {
        failure("PROTON_REAUTH_REQUIRED: authorization rejected")
    } else {
        failure(&format!(
            "Proton API network request failed ({:?})",
            error.kind()
        ))
    }
}
fn stdin_password() -> io::Result<String> {
    let mut value = String::new();
    io::stdin().take(4099).read_to_string(&mut value)?;
    if value.ends_with('\n') {
        value.pop();
    }
    if value.ends_with('\r') {
        value.pop();
    }
    if value.is_empty() || value.len() > 4096 || value.contains(['\r', '\n']) {
        auth::erase(&mut value);
        return Err(failure("Stdin must contain one password line"));
    }
    Ok(value)
}

struct Api<C: Context> {
    session: Session<C>,
    store: LegacyStore,
    sdk: Sdk,
}
impl<C: Context> Api<C> {
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> io::Result<Value> {
        self.store.check()?;
        let method = match method {
            "GET" => muon::http::Method::GET,
            "POST" => muon::http::Method::POST,
            "DELETE" => muon::http::Method::DELETE,
            _ => return Err(failure("Invalid API method")),
        };
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let mut request =
            ProtonRequest::new(method, route).retry_policy(RetryPolicy::default().never());
        for (key, value) in pvpnclient::url::form_urlencoded::parse(query.as_bytes()) {
            request = request.query((key.as_ref(), value.as_ref()));
        }
        for (key, value) in headers {
            request = request.header((*key, *value));
        }
        if let Some(body) = body {
            request = request
                .body_json(body)
                .map_err(|_| failure("Cannot encode API request"))?
        }
        let response = tokio::time::timeout(
            Duration::from_secs(40),
            self.session.send_with_sdk(request, &self.sdk),
        )
        .await
        .map_err(|_| failure("Proton API request timed out"))?
        .map_err(api_error)?;
        self.store.check()?;
        let status = response.status();
        if status == 401 {
            return Err(failure("PROTON_REAUTH_REQUIRED: authorization rejected"));
        }
        if !status.is_success() {
            return Err(failure(&format!(
                "Proton API request failed: HTTP {status}"
            )));
        }
        if response.body().len() > 32 * 1024 * 1024 {
            return Err(failure("Proton API response exceeds size limit"));
        }
        let value: Value = response
            .body_json()
            .map_err(|_| failure("Invalid Proton API response"))?;
        if value["Code"] != 1000 {
            if value["Code"] == 9001 {
                return Err(failure(
                    "PROTON_HUMAN_VERIFICATION_REQUIRED: interactive account verification required",
                ));
            }
            return Err(failure("Proton API rejected the request"));
        }
        Ok(value)
    }
    async fn catalog(&self) -> io::Result<Vec<Value>> {
        let location = self.call("GET", "/vpn/v1/location", None, &[]).await.ok();
        let mut netzone = String::new();
        if let Some(value) = location {
            #[cfg(not(test))]
            if let Some(country) = value["Country"]
                .as_str()
                .filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_uppercase()))
            {
                if let Ok(mut socket) = UnixStream::connect("/var/run/proton-country.sock") {
                    let _ = socket.set_write_timeout(Some(Duration::from_millis(200)));
                    let _ = socket.write_all(format!("set {country}\n").as_bytes());
                }
            }
            if let Some(ip) = value["IP"].as_str().and_then(|s| s.parse::<IpAddr>().ok()) {
                netzone = match ip {
                    IpAddr::V4(ip) => {
                        let mut b = ip.octets();
                        b[3] = 0;
                        Ipv4Addr::from(b).to_string()
                    }
                    IpAddr::V6(ip) => {
                        let mut b = ip.octets();
                        b[6..].fill(0);
                        Ipv6Addr::from(b).to_string()
                    }
                };
            }
        }
        let mut headers = vec![("If-Modified-Since", "Thu, 01 Jan 1970 00:00:00 GMT")];
        if !netzone.is_empty() {
            headers.push(("X-PM-netzone", &netzone));
        }
        let mut result = self
            .call(
                "GET",
                "/vpn/v1/logicals?SecureCoreFilter=all&WithState=true",
                None,
                &headers,
            )
            .await?;
        match result.get_mut("LogicalServers").map(Value::take) {
            Some(Value::Array(nodes)) => Ok(nodes),
            _ => Err(failure("Server catalog unavailable")),
        }
    }
    async fn revoke(&self, serial: &str) -> io::Result<()> {
        if serial.is_empty() || !serial.bytes().all(|b| b.is_ascii_digit()) {
            return Err(failure("Invalid certificate serial"));
        }
        let response = self
            .call(
                "DELETE",
                "/vpn/v1/certificate",
                Some(json!({"SerialNumber":serial})),
                &[],
            )
            .await?;
        if response["Count"] != 1 {
            return Err(failure("Certificate revocation was not confirmed"));
        }
        Ok(())
    }
    async fn certificates(&self, with_sessions: bool) -> io::Result<Vec<Value>> {
        let mut result = Vec::new();
        let mut begin = String::new();
        let mut seen = HashSet::new();
        for _ in 0..10000 {
            let path = format!(
                "/vpn/v1/certificate/all?{}&Limit=50{}",
                if with_sessions {
                    "WithSessions=1"
                } else {
                    "Mode=persistent"
                },
                if begin.is_empty() {
                    String::new()
                } else {
                    format!("&BeginID={begin}")
                }
            );
            let response = self.call("GET", &path, None, &[]).await?;
            let page = response["Certificates"]
                .as_array()
                .ok_or_else(|| failure("Invalid certificate list"))?;
            result.extend(page.iter().cloned());
            if page.len() < 50 {
                return Ok(result);
            }
            begin = page
                .last()
                .and_then(|c| c["SerialNumber"].as_str())
                .unwrap_or("")
                .to_owned();
            if !begin.bytes().all(|b| b.is_ascii_digit())
                || begin.is_empty()
                || !seen.insert(begin.clone())
            {
                return Err(failure("Invalid certificate pagination"));
            }
        }
        Err(failure("Certificate page limit exceeded"))
    }
}

fn num(value: &Value, key: &str) -> i64 {
    value[key].as_i64().unwrap_or(0)
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn matching(server: &Value, o: &Options) -> bool {
    if num(server, "Status") != 1
        || server["Servers"].as_array().map_or(true, Vec::is_empty)
        || o.flag("free-only") != (num(server, "Tier") == 0)
    {
        return false;
    }
    if !o.text("countries").is_empty()
        && !o
            .text("countries")
            .split(',')
            .any(|c| c == text(server, "ExitCountry"))
    {
        return false;
    }
    let features = num(server, "Features");
    if o.flag("secure-core") && features & 1 == 0 {
        return false;
    }
    if o.flag("p2p-only") && !o.flag("free-only") && !o.flag("secure-core") && features & 4 == 0 {
        return false;
    }
    true
}
fn catalog_rows(all: &[Value], o: &Options) -> io::Result<Vec<Value>> {
    let mut selected = all.iter().filter(|s| matching(s, o)).collect::<Vec<_>>();
    selected.sort_by(|a, b| {
        text(a, "ExitCountry")
            .cmp(text(b, "ExitCountry"))
            .then_with(|| {
                a["Score"]
                    .as_f64()
                    .unwrap_or(0.)
                    .total_cmp(&b["Score"].as_f64().unwrap_or(0.))
                    .then_with(|| {
                        a["Load"]
                            .as_f64()
                            .unwrap_or(0.)
                            .total_cmp(&b["Load"].as_f64().unwrap_or(0.))
                    })
            })
    });
    let mut out = Vec::new();
    for s in selected {
        let features = [
            (1, "SecureCore"),
            (2, "Tor"),
            (4, "P2P"),
            (8, "Streaming"),
            (16, "IPv6"),
        ]
        .iter()
        .filter(|(bit, _)| num(s, "Features") & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
        let endpoints=s["Servers"].as_array().unwrap().iter().map(|p|json!({"hostname":text(p,"Domain"),"entry_ip":text(p,"EntryIP"),"exit_ip":text(p,"ExitIP"),"public_key":text(p,"X25519PublicKey"),"online":num(p,"Status")==1})).collect::<Vec<_>>();
        let mut row = json!({"name":text(s,"Name"),"hostname":text(s,"Domain"),"country":text(s,"ExitCountry"),"entry_country":text(s,"EntryCountry"),"city":text(s,"City"),"tier":match num(s,"Tier"){0=>"Free",2=>"Plus",3=>"ProtonMail",_=>"Unknown"},"score":s["Score"].as_f64().unwrap_or(0.),"load":num(s,"Load"),"features":features,"endpoints":endpoints});
        let host = text(s, "HostCountry");
        if !host.is_empty() && host != text(s, "ExitCountry") {
            row["host_country"] = json!(host)
        }
        out.push(row);
    }
    if out.is_empty() {
        return Err(failure("No online matching servers"));
    }
    Ok(out)
}
fn features(o: &Options) -> Value {
    json!({"NetShieldLevel":o.text("netshield").parse::<u8>().unwrap_or(0),"RandomNAT":!o.flag("moderate-nat"),"PortForwarding":o.flag("port-forwarding"),"SplitTCP":o.flag("accelerator")})
}

async fn operate<C: Context>(api: &Api<C>, o: &Options, path: &Path) -> io::Result<()> {
    if o.flag("logout") {
        api.call("DELETE", "/auth", None, &[]).await?;
        fs::remove_file(path)?;
        return Ok(());
    }
    if !o.text("revoke-serial").is_empty() {
        return api.revoke(o.text("revoke-serial")).await;
    }
    if o.flag("list-configs") {
        let mut rows = Vec::new();
        for cert in api.certificates(o.flag("with-sessions")).await? {
            let mut row = json!({"serial":cert["SerialNumber"],"mode":cert["Mode"],"device_name":cert["DeviceName"],"expires":timestamp(num(&cert,"ExpirationTime"))?,"fingerprint":cert["ClientKeyFingerprint"]});
            if cert["Features"].is_object() {
                let f = &cert["Features"];
                let yes = |k: &str| f[k].as_bool().unwrap_or(num(f, k) != 0);
                row["features"] = json!({"netshield":num(f,"NetShieldLevel"),"moderate_nat":!yes("RandomNAT"),"port_forwarding":yes("PortForwarding"),"accelerator":yes("SplitTCP")});
            }
            rows.push(row)
        }
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if !o.text("renew-serial").is_empty() {
        let certs = api.certificates(false).await?;
        let target = certs
            .iter()
            .find(|c| text(c, "SerialNumber") == o.text("renew-serial"))
            .ok_or_else(|| failure("Persistent credential not found"))?;
        if text(target, "ClientKey").is_empty() || text(target, "DeviceName").is_empty() {
            return Err(failure("Persistent credential metadata incomplete"));
        }
        let fresh = features(o);
        let mut f = if target["Features"].is_object() {
            target["Features"].clone()
        } else {
            fresh.clone()
        };
        for (flag, key) in [
            ("netshield", "NetShieldLevel"),
            ("moderate-nat", "RandomNAT"),
            ("port-forwarding", "PortForwarding"),
            ("accelerator", "SplitTCP"),
        ] {
            if o.explicit.contains(flag) {
                f[key] = fresh[key].clone();
            }
        }
        let result=api.call("POST","/vpn/v1/certificate",Some(json!({"ClientPublicKey":target["ClientKey"],"Duration":format!("{} min",minutes(o.text("duration"))?),"Features":f,"Mode":"persistent","DeviceName":target["DeviceName"],"Renew":true})),&[]).await?;
        if text(&result, "SerialNumber").is_empty() {
            return Err(failure("Renewal returned no serial"));
        }
        println!(
            "Certificate renewed: {} -> {}",
            o.text("renew-serial"),
            text(&result, "SerialNumber")
        );
        return Ok(());
    }
    let all = api.catalog().await?;
    if o.flag("list-servers") {
        let rows = catalog_rows(&all, o)?;
        if o.flag("json") {
            println!("{}", serde_json::to_string_pretty(&rows)?)
        } else {
            for s in rows {
                println!(
                    "{}\t{}\t{}\t{}\t{:.2}",
                    text(&s, "country"),
                    text(&s, "name"),
                    text(&s, "city"),
                    num(&s, "load"),
                    s["score"].as_f64().unwrap_or(0.)
                );
            }
        }
        return Ok(());
    }
    if o.text("server").is_empty() && o.text("countries").is_empty() {
        return Err(failure("Server or countries required"));
    }
    let mut choices = all
        .iter()
        .filter(|s| {
            if !o.text("server").is_empty() {
                num(s, "Status") == 1 && text(s, "Name") == o.text("server")
            } else {
                matching(s, o)
            }
        })
        .collect::<Vec<_>>();
    choices.sort_by(|a, b| {
        a["Score"]
            .as_f64()
            .unwrap_or(0.)
            .total_cmp(&b["Score"].as_f64().unwrap_or(0.))
            .then_with(|| {
                a["Load"]
                    .as_f64()
                    .unwrap_or(0.)
                    .total_cmp(&b["Load"].as_f64().unwrap_or(0.))
            })
    });
    let server = choices
        .first()
        .ok_or_else(|| failure("No online matching server"))?;
    let peer = server["Servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| num(p, "Status") == 1)
        .ok_or_else(|| failure("No online physical server"))?;
    let endpoint = text(peer, "EntryIP")
        .parse::<IpAddr>()
        .map_err(|_| failure("Invalid server address"))?;
    if STANDARD
        .decode(text(peer, "X25519PublicKey"))
        .map_err(|_| failure("Invalid peer key"))?
        .len()
        != 32
    {
        return Err(failure("Invalid peer key"));
    }
    let private = pvpnclient::util::generate_ed25519_private_key(rand::rng());
    let (public, wg, _) = pvpnclient::util::derive_keys_from_ed25519_private_key(&private);
    let mut body = json!({"ClientPublicKey":public.pem,"Duration":format!("{} min",minutes(o.text("duration"))?),"Features":features(o)});
    if !o.flag("no-save") {
        body["Mode"] = json!("persistent");
        body["DeviceName"] = json!(o.text("device-name"));
    }
    let cert = api
        .call("POST", "/vpn/v1/certificate", Some(body), &[])
        .await?;
    let serial = text(&cert, "SerialNumber");
    if serial.is_empty() || num(&cert, "ExpirationTime") <= now() {
        return Err(failure("Invalid certificate metadata"));
    }
    let dns = if !o.text("dns").is_empty() {
        o.text("dns").replace(',', ", ")
    } else if o.flag("ipv6") {
        "10.2.0.1, 2a07:b944::2:1".into()
    } else {
        "10.2.0.1".into()
    };
    let allowed = if !o.text("allowed-ips").is_empty() {
        o.text("allowed-ips").replace(',', ", ")
    } else if o.flag("ipv6") {
        "0.0.0.0/0, ::/0".into()
    } else {
        "0.0.0.0/0".into()
    };
    let address = if o.flag("ipv6") {
        "10.2.0.2/32, 2a07:b944::2:2/128"
    } else {
        "10.2.0.2/32"
    };
    let endpoint = std::net::SocketAddr::new(endpoint, 51820);
    let content = format!(
        "# Proton VPN controller credential\n[Interface]\nPrivateKey = {}\nAddress = {address}\nDNS = {dns}\n\n[Peer]\nPublicKey = {}\nAllowedIPs = {allowed}\nEndpoint = {endpoint}\n",
        STANDARD.encode(wg.key),
        text(peer, "X25519PublicKey")
    );
    if atomic_write(Path::new(o.text("output")), content.as_bytes()).is_err() {
        if api.revoke(serial).await.is_err() {
            return Err(failure(&format!(
                "Configuration write failed; pending cleanup serial {serial}"
            )));
        }
        return Err(failure(
            "Configuration write failed; generated credential revoked",
        ));
    }
    let mode = cert["Mode"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("session");
    let receipt = if o.flag("metadata-json") {
        json!({"version":1,"serial":serial,"expires":num(&cert,"ExpirationTime"),"mode":mode,"server":text(server,"Name"),"endpoint":endpoint.to_string()}).to_string()+"\n"
    } else {
        format!(
            "Certificate: {mode}, serial {serial}, expires {}\n",
            timestamp(num(&cert, "ExpirationTime"))?
        )
    };
    // A successful private config write owns a credential even if stdout fails.
    // The transaction's receipt/ownership contract remains authoritative.
    if io::stdout().write_all(receipt.as_bytes()).is_err() {
        let _ = fs::remove_file(o.text("output"));
        if api.revoke(serial).await.is_err() {
            return Err(failure(&format!(
                "Receipt write failed; pending cleanup serial {serial}"
            )));
        }
        return Err(failure("Credential receipt could not be delivered"));
    }
    Ok(())
}

pub fn main(args: &[String], revoke_alias: bool) -> io::Result<()> {
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!(
            "Rust Proton account compatibility API: --username USER --list-servers --free-only --json; --server NAME --ipv6 --no-save --duration 7d --metadata-json --output FILE; --revoke-serial SERIAL; --list-configs; --renew-serial SERIAL; --logout; --forget. Existing controller flags remain supported."
        );
        return Ok(());
    }
    if unsafe { libc::geteuid() } != 0 {
        return Err(failure("Run account commands as root"));
    }
    let mut o = Options::parse(args)?;
    if revoke_alias {
        o.values.insert("logout".into(), "true".into());
    }
    let dir = std::env::var_os("PROTON_SESSION_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .ok_or_else(|| failure("Authorization directory unavailable"))?;
    let path = dir.join(".protonvpn-session.json");
    if o.flag("forget") {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        return Ok(());
    }
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join(".proton-account.guard"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(failure("Another account operation is running"));
    }
    if o.flag("clear-session") {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let mut saved = if o.flag("no-session") {
        json!({})
    } else {
        match private_read(&path) {
            Ok(raw) => serde_json::from_slice::<Value>(&raw)
                .map_err(|_| failure("Invalid saved authorization"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e),
        }
    };
    if o.flag("logout") && saved.get("session").is_none() {
        return Ok(());
    }
    if o.text("username").is_empty() && o.flag("logout") {
        o.values
            .insert("username".into(), text(&saved, "username").into());
    }
    if o.text("username").is_empty() {
        if o.flag("non-interactive") {
            return Err(failure("PROTON_REAUTH_REQUIRED: account identity missing"));
        }
        o.values
            .insert("username".into(), auth::prompt("Username: ", false)?);
    }
    if o.text("username").is_empty() {
        return Err(failure("Account identity required"));
    }
    if saved.get("session").is_some() && text(&saved, "username") != o.text("username") {
        return Err(failure("Authorization identity mismatch"));
    }
    if !saved.is_object() {
        return Err(failure("Invalid authorization object"));
    }
    saved["username"] = json!(o.text("username"));
    if saved.get("session").is_none() && o.flag("logout") {
        return Ok(());
    }
    if saved.get("session").is_none() && o.flag("non-interactive") {
        return Err(failure(
            "PROTON_REAUTH_REQUIRED: saved authorization unavailable",
        ));
    }
    let store = LegacyStore::new(
        if o.flag("no-session") {
            None
        } else {
            Some(path.clone())
        },
        saved,
    )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async{
        let app=App::new("linux-vpn-gui@5.8.7+aarch64").map_err(|_|failure("Invalid Proton app identity"))?.with_user_agent("ProtonVPN/5.8.7 (Linux; ubuntu/24.04)");
        let sdk=Sdk::new("protun-openwrt-account",env!("CARGO_PKG_VERSION")).map_err(|_|failure("Invalid SDK identity"))?;
        let mut headers=vec![("Accept".to_owned(),"application/json".to_owned())];
        if !o.text("hv-token").is_empty(){headers.push(("x-pm-human-verification-token".into(),o.text("hv-token").into()));headers.push(("x-pm-human-verification-token-type".into(),"captcha".into()));}
        let client=muon::Client::builder(app,environment(o.text("api-url"),std::env::var_os("PROTON_SESSION_DIR").is_some())?).with_operating_system(account_io::from_environment()?,rand::rng()).with_multi_thread_executor(auth::Executor).with_persistence(store.clone()).retry_policy(RetryPolicy::default().never()).without_cookie_store().with_default_headers(headers).register_sdk(sdk.clone()).build().map_err(|_|failure("Cannot initialize Proton account client"))?;
        let session=if let Some(session)=client.get_session(()).await{session}else{
            let session=client.new_session_without_credentials(()).await.map_err(|_|failure("Cannot initialize Proton login"))?;
            let mut password=if o.flag("password-stdin"){stdin_password()?}else if !o.text("password").is_empty(){o.text("password").into()}else{auth::prompt("Password: ",true)?};
            let flow=tokio::time::timeout(Duration::from_secs(120),session.auth().login(o.text("username"),&password)).await;auth::erase(&mut password);
            match flow.map_err(|_|failure("Proton login timed out"))?{LoginFlow::Ok(s,_)=>s,LoginFlow::TwoFactor(flow,_)=>{if !flow.has_totp(){return Err(failure("Unsupported two-factor method; account remains unauthorized"))}let mut code=if !o.text("fixture-totp").is_empty(){o.text("fixture-totp").into()}else{auth::prompt("2FA Code: ",true)?};if code.len()!=6 || !code.bytes().all(|b|b.is_ascii_digit()){auth::erase(&mut code);return Err(failure("Six digit TOTP required"))}
                let result=tokio::time::timeout(Duration::from_secs(60),flow.totp(&code)).await;auth::erase(&mut code);result.map_err(|_|failure("Two-factor login timed out"))?.map_err(|_|failure("Two-factor authentication failed"))?},LoginFlow::Failed{..}=>return Err(failure("Proton authentication failed; check credentials and account verification requirements"))}
        };
        store.check()?;
        if !session.has_scope("vpn").await&&!session.has_scope("full").await{return Err(failure("PROTON_REAUTH_REQUIRED: VPN authorization scope missing"))}
        if o.flag("force-refresh") || o.text("session-duration")!="0" {session.refresh_auth().await.map_err(|_|failure("Proton authorization refresh failed"))?;store.check()?;}
        let api=Api{session,store,sdk};operate(&api,&o,&path).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(args: &[&str]) -> Options {
        Options::parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }
    #[test]
    fn legacy_controller_options_and_limits() {
        let o = options(&[
            "--username",
            "fixture",
            "--non-interactive",
            "--server",
            "US-FREE#1",
            "--free-only",
            "--ipv6",
            "--no-save",
            "--duration",
            "7d",
            "--metadata-json",
            "--output",
            "pending.conf",
        ]);
        assert!(o.flag("metadata-json"));
        assert_eq!(minutes(o.text("duration")).unwrap(), 10080);
        for args in [
            vec!["--non-interactive", "--password", "fixture"],
            vec!["--port-forwarding", "--moderate-nat"],
            vec!["--netshield", "3"],
            vec!["--no-save", "--duration", "8d"],
            vec!["--countries", "U1"],
            vec!["--dns", "bad"],
        ] {
            assert!(
                Options::parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err()
            );
        }
    }
    #[test]
    fn catalog_retains_cpp_contract() {
        let server = json!({"Name":"US-FREE#1","Status":1,"Tier":0,"Features":16,"ExitCountry":"US","EntryCountry":"US","City":"Example","Score":2.5,"Load":30,"Servers":[{"Status":1,"EntryIP":"192.0.2.1","X25519PublicKey":"example"}]});
        let rows = catalog_rows(&[server], &options(&["--free-only"])).unwrap();
        assert_eq!(rows[0]["tier"], "Free");
        assert_eq!(rows[0]["features"], json!(["IPv6"]));
        assert_eq!(rows[0]["endpoints"][0]["online"], true);
    }
    #[test]
    fn legacy_session_import_and_unchanged_auth_do_not_write() {
        let saved = json!({"username":"fixture","session":{"UID":"fixture-id","AccessToken":"fixture-access","RefreshToken":"fixture-refresh","Scopes":["vpn"]}});
        let s = LegacyStore::new(None, saved.clone()).unwrap();
        let a = s.get_all_auth().remove(&()).unwrap();
        s.persist(&a).unwrap();
        assert_eq!(*s.value.lock().unwrap(), saved);
        assert!(!format!("{s:?}").contains("fixture-access"));
    }
    #[test]
    fn anonymous_login_does_not_overwrite_authorization() {
        let saved = json!({"username":"fixture"});
        let s = LegacyStore::new(None, saved.clone()).unwrap();
        s.persist(&Auth::anonymous(
            "anonymous",
            muon::auth::Tokens::access("a", "r", Vec::<String>::new()),
        ))
        .unwrap();
        assert_eq!(*s.value.lock().unwrap(), saved);
    }
}

#[cfg(test)]
#[path = "openwrt-account-tests.rs"]
mod integration_tests;

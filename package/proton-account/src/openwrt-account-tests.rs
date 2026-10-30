// SPDX-License-Identifier: GPL-3.0-or-later
// Loopback-only integration tests; these never use real account authorization.
use super::*;
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
    thread,
    time::Instant,
};

fn server(
    mut response: impl FnMut(&str, &Value) -> (u16, Value) + Send + 'static,
    count: usize,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let join = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut calls = Vec::new();
        while calls.len() < count {
            assert!(
                Instant::now() < deadline,
                "fixture request deadline exceeded"
            );
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("fixture listener: {e}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut length = 0;
            let mut authorized = false;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    match key.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap(),
                        "authorization" => authorized = value.trim() == "Bearer fixture-access",
                        _ => {}
                    }
                }
            }
            assert!(authorized, "imported authorization was not sent");
            assert!(length < 1024 * 1024);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            drop(reader);
            let value = if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            };
            let request = first
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            let (status, value) = response(&request, &value);
            let bytes = value.to_string();
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}",bytes.len()).unwrap();
            calls.push(request);
        }
        calls
    });
    (url, join)
}
async fn api(url: &str) -> io::Result<Api<impl Context<SessionKey = ()>>> {
    let saved = json!({"username":"fixture","session":{"UID":"fixture-id","AccessToken":"fixture-access","RefreshToken":"fixture-refresh","Scopes":["vpn"]}});
    let store = LegacyStore::new(None, saved)?;
    let sdk = Sdk::new("protun-openwrt-account-test", "1.0.0").unwrap();
    let os = account_io::Os {
        clock: Default::default(),
        dialer: Default::default(),
        resolver: Default::default(),
    };
    let client = muon::Client::builder(
        App::new("linux-vpn-gui@5.8.7+aarch64").unwrap(),
        environment(url, true)?,
    )
    .with_operating_system(os, rand::rng())
    .with_multi_thread_executor(auth::Executor)
    .with_persistence(store.clone())
    .retry_policy(RetryPolicy::default().never())
    .without_cookie_store()
    .register_sdk(sdk.clone())
    .build()
    .map_err(api_error)?;
    let session = client
        .get_session(())
        .await
        .ok_or_else(|| failure("Fixture session import failed"))?;
    Ok(Api {
        session,
        store,
        sdk,
    })
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn options(args: &[&str]) -> Options {
    Options::parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
}
fn fixture_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "protun-account-test-{}-{}",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn duration_boundaries_match_legacy_contract() {
    for (raw, expected) in [("7d", 10080), ("1h30m", 90), (".5h", 30), ("10m", 10)] {
        assert_eq!(minutes(raw).unwrap(), expected)
    }
    for raw in [
        "0d",
        "9m",
        "10m1ns",
        "1ms",
        "366d",
        "999999999999999999999h",
    ] {
        assert!(minutes(raw).is_err(), "{raw}")
    }
}
#[test]
fn fixture_endpoints_cannot_redirect_real_api_requests() {
    assert!(matches!(
        environment("https://vpn-api.proton.me", false).unwrap(),
        Environment::Prod(_)
    ));
    assert!(environment("http://127.0.0.1:12345", true).is_ok());
    for raw in [
        "http://127.0.0.1:12345",
        "https://example.com",
        "https://vpn-api.proton.me.evil.test",
        "http://user@127.0.0.1:12345",
        "http://127.0.0.1:12345/?secret=x",
    ] {
        assert!(environment(raw, false).is_err())
    }
    assert!(environment("https://example.com", true).is_err());
}
#[test]
fn failed_logout_preserves_saved_authorization() {
    let path = fixture_path();
    atomic_write(&path, b"fixture authorization").unwrap();
    let (url, server) = server(
        |request, _| {
            assert_eq!(request, "DELETE /auth");
            (403, json!({"Code":2011}))
        },
        1,
    );
    runtime().block_on(async {
        let api = api(&url).await.unwrap();
        assert!(operate(&api, &options(&["--logout"]), &path).await.is_err())
    });
    assert_eq!(fs::read(&path).unwrap(), b"fixture authorization");
    fs::remove_file(path).unwrap();
    assert_eq!(server.join().unwrap().len(), 1);
}
#[test]
fn revocation_requires_confirmation() {
    let (url, server) = server(
        |request, body| {
            assert_eq!(request, "DELETE /vpn/v1/certificate");
            assert_eq!(body["SerialNumber"], "777");
            (200, json!({"Code":1000,"Count":0}))
        },
        1,
    );
    runtime().block_on(async { assert!(api(&url).await.unwrap().revoke("777").await.is_err()) });
    server.join().unwrap();
}
#[test]
fn failed_config_write_revokes_only_the_new_credential() {
    let missing = fixture_path().join("missing.conf");
    let (url, server) = server(
        |request, body| match request {
            "GET /vpn/v1/location" => (200, json!({"Code":1000,"Country":"US","IP":"192.0.2.9"})),
            "GET /vpn/v1/logicals?SecureCoreFilter=all&WithState=true" => (
                200,
                json!({"Code":1000,"LogicalServers":[{"Name":"US-FREE#1","Status":1,"Tier":0,"Features":16,"Servers":[{"Status":1,"EntryIP":"192.0.2.1","X25519PublicKey":STANDARD.encode([1u8;32])}]}]}),
            ),
            "POST /vpn/v1/certificate" => {
                assert_eq!(body["Duration"], "10080 min");
                assert!(
                    body["ClientPublicKey"]
                        .as_str()
                        .unwrap()
                        .starts_with("-----BEGIN PUBLIC KEY-----")
                );
                (
                    200,
                    json!({"Code":1000,"SerialNumber":"777","ExpirationTime":now()+3600}),
                )
            }
            "DELETE /vpn/v1/certificate" => {
                assert_eq!(body["SerialNumber"], "777");
                (200, json!({"Code":1000,"Count":1}))
            }
            _ => panic!("Unexpected fixture request: {request}"),
        },
        4,
    );
    runtime().block_on(async {
        let api = api(&url).await.unwrap();
        let o = options(&[
            "--server",
            "US-FREE#1",
            "--free-only",
            "--ipv6",
            "--no-save",
            "--output",
            missing.to_str().unwrap(),
        ]);
        let e = operate(&api, &o, Path::new("unused")).await.unwrap_err();
        assert!(e.to_string().contains("generated credential revoked"));
    });
    let calls = server.join().unwrap();
    assert_eq!(calls.last().unwrap(), "DELETE /vpn/v1/certificate");
    assert!(!missing.exists());
}
#[test]
fn persistence_failure_is_observable_and_does_not_claim_success() {
    let path = fixture_path().join("session.json");
    let mut store = LegacyStore::new(Some(path), json!({"username":"fixture"})).unwrap();
    store.set_auth(
        (),
        Auth::anonymous(
            "fixture-id",
            muon::auth::Tokens::access(
                "fixture-access",
                "fixture-refresh",
                vec!["vpn".to_string()],
            ),
        ),
    );
    assert!(store.check().is_err());
    assert!(store.value.lock().unwrap().get("session").is_none());
}

#[test]
fn size_build_preserves_certificate_listing_with_sessions() {
    let (url, server) = server(|request, _| {
        assert_eq!(request, "GET /vpn/v1/certificate/all?WithSessions=1&Limit=50");
        (200, json!({"Code":1000,"Certificates":[{"SerialNumber":"777","Mode":"persistent","DeviceName":"fixture-device","ExpirationTime":now()+3600,"ClientKeyFingerprint":"fixture-fingerprint","Features":{"NetShieldLevel":2,"RandomNAT":false,"PortForwarding":true,"SplitTCP":true}}]}))
    },1);
    runtime().block_on(async {
        let api=api(&url).await.unwrap();
        operate(&api,&options(&["--list-configs","--with-sessions"]),Path::new("unused")).await.unwrap();
    });
    assert_eq!(server.join().unwrap().len(),1);
}

#[test]
fn size_build_preserves_persistent_renewal_features() {
    let (url, server) = server(|request,body| match request {
        "GET /vpn/v1/certificate/all?Mode=persistent&Limit=50" => (200,json!({"Code":1000,"Certificates":[{"SerialNumber":"777","ClientKey":"fixture-public-key","DeviceName":"fixture-device","Features":{"NetShieldLevel":2,"RandomNAT":false,"PortForwarding":true,"SplitTCP":false,"FutureFeature":true}}]})),
        "POST /vpn/v1/certificate" => {
            assert_eq!(body["ClientPublicKey"],"fixture-public-key");
            assert_eq!(body["DeviceName"],"fixture-device");
            assert_eq!(body["Mode"],"persistent");
            assert_eq!(body["Duration"],"43200 min");
            assert_eq!(body["Renew"],true);
            assert_eq!(body["Features"]["NetShieldLevel"],2);
            assert_eq!(body["Features"]["SplitTCP"],false);
            assert_eq!(body["Features"]["FutureFeature"],true);
            (200,json!({"Code":1000,"SerialNumber":"778"}))
        },
        _ => panic!("Unexpected fixture request: {request}"),
    },2);
    runtime().block_on(async {
        let api=api(&url).await.unwrap();
        operate(&api,&options(&["--renew-serial","777","--duration","30d"]),Path::new("unused")).await.unwrap();
    });
    assert_eq!(server.join().unwrap().len(),2);
}

#[test]
fn size_build_preserves_named_persistent_credentials_and_manual_overrides() {
    use std::os::unix::fs::PermissionsExt;
    let (url, server) = server(|request,body| match request {
        "GET /vpn/v1/location" => (200,json!({"Code":1000,"Country":"US","IP":"192.0.2.9"})),
        "GET /vpn/v1/logicals?SecureCoreFilter=all&WithState=true" => (200,json!({"Code":1000,"LogicalServers":[{"Name":"US-PLUS#1","Status":1,"Tier":2,"Features":20,"Servers":[{"Status":1,"EntryIP":"192.0.2.1","X25519PublicKey":STANDARD.encode([1u8;32])}]}]})),
        "POST /vpn/v1/certificate" => {
            assert_eq!(body["Mode"],"persistent");
            assert_eq!(body["DeviceName"],"future-router");
            assert_eq!(body["Duration"],"43200 min");
            assert_eq!(body["Features"]["NetShieldLevel"],2);
            assert_eq!(body["Features"]["RandomNAT"],true);
            assert_eq!(body["Features"]["PortForwarding"],true);
            (200,json!({"Code":1000,"SerialNumber":"777","ExpirationTime":now()+3600,"Mode":"persistent"}))
        },
        _ => panic!("Unexpected fixture request: {request}"),
    },3);
    let directory=fixture_path();fs::create_dir(&directory).unwrap();fs::set_permissions(&directory,fs::Permissions::from_mode(0o700)).unwrap();
    let output=directory.join("persistent.conf");
    runtime().block_on(async {
        let api=api(&url).await.unwrap();
        let o=options(&["--server","US-PLUS#1","--device-name","future-router","--duration","30d","--ipv6","--dns","10.2.0.1,2a07:b944::2:1","--allowed-ips","192.0.2.0/24,2001:db8::/32","--netshield","2","--port-forwarding","--metadata-json","--output",output.to_str().unwrap()]);
        operate(&api,&o,Path::new("unused")).await.unwrap();
    });
    let content=fs::read_to_string(&output).unwrap();
    assert!(content.contains("DNS = 10.2.0.1, 2a07:b944::2:1"));
    assert!(content.contains("AllowedIPs = 192.0.2.0/24, 2001:db8::/32"));
    assert!(content.contains("Address = 10.2.0.2/32, 2a07:b944::2:2/128"));
    assert_eq!(fs::metadata(&output).unwrap().permissions().mode()&0o777,0o600);
    fs::remove_file(output).unwrap();fs::remove_dir(directory).unwrap();
    assert_eq!(server.join().unwrap().len(),3);
}

#[test]
fn size_build_preserves_advanced_server_filters() {
    let secure=json!({"Name":"CH-PLUS#1","Status":1,"Tier":2,"Features":1,"ExitCountry":"CH","Servers":[{}]});
    let regular=json!({"Name":"US-PLUS#1","Status":1,"Tier":2,"Features":4,"ExitCountry":"US","Servers":[{}]});
    let o=options(&["--countries","CH","--secure-core"]);
    assert!(matching(&secure,&o));assert!(!matching(&regular,&o));
    let p2p=options(&["--p2p-only"]);assert!(matching(&regular,&p2p));assert!(!matching(&secure,&p2p));
    let nat=options(&["--moderate-nat"]);assert_eq!(features(&nat)["RandomNAT"],false);
}


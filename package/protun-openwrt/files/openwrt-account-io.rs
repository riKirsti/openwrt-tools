// SPDX-License-Identifier: GPL-3.0-or-later
// Account API sockets follow the existing controller's underlay mark contract.
use super::{auth, failure};
use async_compat::Compat;
use hickory_proto::{
    op::Message,
    rr::{Name, RData, RecordType},
};
use muon::rt::{OperatingSystem, Resolve, TcpConnect};
use std::{
    io,
    net::{IpAddr, SocketAddr},
    os::fd::AsRawFd,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, TcpStream, UdpSocket},
};

#[derive(Clone, Debug, Default)]
pub(super) struct Dialer {
    pub mark: Option<u32>,
}
fn mark(fd: i32, value: Option<u32>) -> io::Result<()> {
    if let Some(value) = value {
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_MARK,
                (&value as *const u32).cast(),
                4,
            )
        } != 0
        {
            return Err(failure("Cannot protect account API socket"));
        }
    }
    Ok(())
}
impl Dialer {
    async fn connect(&self, addr: SocketAddr) -> io::Result<TcpStream> {
        let socket = if addr.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        mark(socket.as_raw_fd(), self.mark)?;
        tokio::time::timeout(Duration::from_secs(10), socket.connect(addr))
            .await
            .map_err(|_| failure("Account connection timed out"))?
    }
}
impl TcpConnect for Dialer {
    type Socket = Compat<TcpStream>;
    type Err = io::Error;
    async fn tcp_connect(&self, addr: SocketAddr) -> io::Result<Self::Socket> {
        Ok(Compat::new(self.connect(addr).await?))
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Resolver {
    pub dns: Option<SocketAddr>,
    pub dialer: Dialer,
}
impl Resolver {
    async fn query(&self, host: &str, kind: RecordType) -> io::Result<Vec<IpAddr>> {
        use hickory_proto::op::Query;
        let name = Name::from_ascii(host).map_err(|_| failure("Invalid API hostname"))?;
        let id = rand::random::<u16>();
        let mut query = Message::new(
            id,
            hickory_proto::op::MessageType::Query,
            hickory_proto::op::OpCode::Query,
        );
        query.metadata.recursion_desired = true;
        query.queries.push(Query::query(name, kind));
        let wire = query
            .to_vec()
            .map_err(|_| failure("Cannot encode bootstrap DNS query"))?;
        let dns = self.dns.ok_or_else(|| failure("Bootstrap DNS missing"))?;
        let raw = std::net::UdpSocket::bind(if dns.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
        mark(raw.as_raw_fd(), self.dialer.mark)?;
        raw.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(raw)?;
        socket.connect(dns).await?;
        socket.send(&wire).await?;
        let mut bytes = vec![0; 65535];
        let n = tokio::time::timeout(Duration::from_secs(5), socket.recv(&mut bytes))
            .await
            .map_err(|_| failure("Bootstrap DNS timed out"))??;
        bytes.truncate(n);
        let mut answer =
            Message::from_vec(&bytes).map_err(|_| failure("Invalid bootstrap DNS response"))?;
        if answer.metadata.id != id || answer.queries != query.queries {
            return Err(failure("Bootstrap DNS query mismatch"));
        }
        if answer.metadata.truncation {
            let mut tcp = self.dialer.connect(dns).await?;
            tcp.write_all(&(wire.len() as u16).to_be_bytes()).await?;
            tcp.write_all(&wire).await?;
            let n = tcp.read_u16().await? as usize;
            bytes.resize(n, 0);
            tcp.read_exact(&mut bytes).await?;
            answer = Message::from_vec(&bytes)
                .map_err(|_| failure("Invalid bootstrap DNS TCP response"))?;
            if answer.metadata.id != id
                || answer.queries != query.queries
                || answer.metadata.truncation
            {
                return Err(failure("Bootstrap DNS TCP query mismatch"));
            }
        }
        let mut out = Vec::new();
        for rr in &answer.answers {
            match &rr.data {
                RData::A(a) => out.push(IpAddr::V4(a.0)),
                RData::AAAA(a) => out.push(IpAddr::V6(a.0)),
                _ => {}
            }
        }
        Ok(out)
    }
}
impl Resolve for Resolver {
    type Err = io::Error;
    async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        if let Ok(ip) = host.parse() {
            return Ok(vec![ip]);
        }
        if self.dns.is_none() {
            return Ok(tokio::net::lookup_host((host, 443))
                .await?
                .map(|a| a.ip())
                .collect());
        }
        let results = futures::join!(
            self.query(host, RecordType::A),
            self.query(host, RecordType::AAAA)
        );
        let mut out = Vec::new();
        for result in [results.0, results.1] {
            if let Ok(ips) = result {
                out.extend(ips)
            }
        }
        out.sort();
        out.dedup();
        if out.is_empty() {
            return Err(failure("Bootstrap DNS resolution failed"));
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub(super) struct Os {
    pub clock: auth::Clock,
    pub dialer: Dialer,
    pub resolver: Resolver,
}
impl OperatingSystem for Os {
    type Time = auth::Clock;
    type TcpConnector = Dialer;
    type Resolver = Resolver;
    fn get_time_capabilities(&self) -> &auth::Clock {
        &self.clock
    }
    fn get_tcp_connector(&self) -> &Dialer {
        &self.dialer
    }
    fn get_resolver(&self) -> &Resolver {
        &self.resolver
    }
}
pub(super) fn from_environment() -> io::Result<Os> {
    let value = std::env::var("PROTON_WAN_MARK").unwrap_or_default();
    let mark = if value.is_empty() {
        None
    } else {
        let parsed = if let Some(v) = value.strip_prefix("0x") {
            u32::from_str_radix(v, 16)
        } else {
            value.parse()
        }
        .map_err(|_| failure("Invalid control-plane mark"))?;
        if parsed == 0 {
            return Err(failure("Invalid control-plane mark"));
        }
        Some(parsed)
    };
    let raw = std::env::var("PROTON_WAN_DNS").unwrap_or_default();
    let dns = if raw.is_empty() {
        None
    } else {
        let (addr, scope) = raw.split_once('%').unwrap_or((&raw, ""));
        let ip = addr
            .parse::<IpAddr>()
            .map_err(|_| failure("Invalid bootstrap DNS"))?;
        let scope = if scope.is_empty() {
            0
        } else if let Ok(n) = scope.parse::<u32>() {
            n
        } else {
            let name =
                std::ffi::CString::new(scope).map_err(|_| failure("Invalid DNS interface"))?;
            let n = unsafe { libc::if_nametoindex(name.as_ptr()) };
            if n == 0 {
                return Err(failure("Invalid DNS interface"));
            }
            n
        };
        Some(match ip {
            IpAddr::V4(ip) => SocketAddr::new(ip.into(), 53),
            IpAddr::V6(ip) => std::net::SocketAddrV6::new(ip, 53, 0, scope).into(),
        })
    };
    let dialer = Dialer { mark };
    Ok(Os {
        clock: Default::default(),
        dialer: dialer.clone(),
        resolver: Resolver { dns, dialer },
    })
}

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
        let mut name = Name::from_ascii(host).map_err(|_| failure("Invalid API hostname"))?;
        // A wire-decoded DNS question is absolute, including its root label.
        // Name equality deliberately distinguishes relative and absolute names.
        name.set_fqdn(true);
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

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{
        op::{MessageType, OpCode},
        rr::{
            Record,
            rdata::{A, AAAA},
        },
    };
    use std::net::{Ipv4Addr, Ipv6Addr};
    fn answer(query: &[u8]) -> Vec<u8> {
        let query = Message::from_vec(query).unwrap();
        let mut response = Message::new(query.metadata.id, MessageType::Response, OpCode::Query);
        response.queries = query.queries.clone();
        let q = &query.queries[0];
        let data = match q.query_type {
            RecordType::A => RData::A(A(Ipv4Addr::new(192, 0, 2, 7))),
            RecordType::AAAA => RData::AAAA(AAAA("2001:db8::7".parse::<Ipv6Addr>().unwrap())),
            _ => panic!("unexpected DNS type"),
        };
        response
            .answers
            .push(Record::from_rdata(q.name.clone(), 60, data));
        response.to_vec().unwrap()
    }
    #[test]
    fn relative_api_hostname_matches_wire_dns_questions() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let thread = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut buf = [0u8; 4096];
                let (n, peer) = socket.recv_from(&mut buf).unwrap();
                socket.send_to(&answer(&buf[..n]), peer).unwrap();
            }
        });
        let resolver = Resolver {
            dns: Some(addr),
            dialer: Dialer::default(),
        };
        let ips = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolver.resolve("example.invalid"))
            .unwrap();
        thread.join().unwrap();
        assert!(ips.contains(&"192.0.2.7".parse().unwrap()));
        assert!(ips.contains(&"2001:db8::7".parse().unwrap()));
    }
    #[test]
    fn truncated_udp_answer_retries_bootstrap_dns_over_tcp() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let udp = std::net::UdpSocket::bind(addr).unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        listener.set_nonblocking(true).unwrap();
        let thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let (n, peer) = udp.recv_from(&mut buf).unwrap();
            let mut reply = Message::from_vec(&answer(&buf[..n])).unwrap();
            reply.metadata.truncation = true;
            reply.answers.clear();
            udp.send_to(&reply.to_vec().unwrap(), peer).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("DNS TCP fixture failed: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut size = [0; 2];
            stream.read_exact(&mut size).unwrap();
            let mut body = vec![0; u16::from_be_bytes(size) as usize];
            stream.read_exact(&mut body).unwrap();
            let reply = answer(&body);
            stream
                .write_all(&(reply.len() as u16).to_be_bytes())
                .unwrap();
            stream.write_all(&reply).unwrap();
        });
        let resolver = Resolver {
            dns: Some(addr),
            dialer: Dialer::default(),
        };
        let ips = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(resolver.query("example.invalid", RecordType::A))
            .unwrap();
        thread.join().unwrap();
        assert_eq!(ips, vec!["192.0.2.7".parse::<IpAddr>().unwrap()]);
    }
    #[test]
    fn failure_to_mark_a_socket_is_not_silently_ignored() {
        assert!(mark(-1, Some(0x621)).is_err());
    }
}

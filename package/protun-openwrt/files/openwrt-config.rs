// SPDX-License-Identifier: GPL-3.0-or-later
use super::{decode_key, failure, interface_name, read_file};
use protun::api::connection::{
    CertificateDuration, ConnectionMode, InitialConnectionConfig, IpAddress, MuonEnv, PeerInfo,
    SniStrategy, WgClientPrivateKey, WgPeerPublicKey,
};
use protun::api::local_agent::{LocalAgentSettings, NetshieldLevel};
use protun::api::state::Protocol;
use serde::Deserialize;
use std::{io, net::IpAddr, path::Path};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ProtocolChoice {
    #[default]
    Stealth,
    WireguardUdp,
    WireguardTcp,
    Auto,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SniChoice {
    #[default]
    Random,
    Top,
}
impl ProtocolChoice {
    pub fn allows(self, protocol: Protocol) -> bool {
        match self {
            Self::Stealth => protocol == Protocol::Stealth,
            Self::WireguardUdp => protocol == Protocol::WireguardUdp,
            Self::WireguardTcp => protocol == Protocol::WireguardTcp,
            Self::Auto => true,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub id: String,
    pub server_ip: IpAddr,
    pub server_public_key: String,
    #[serde(default)]
    pub udp_ports: Vec<u16>,
    #[serde(default)]
    pub tcp_ports: Vec<u16>,
    #[serde(default)]
    pub tls_ports: Vec<u16>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub exit_label: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub split_tcp: Option<bool>,
    pub netshield_level: Option<u8>,
    pub soft_jail: Option<bool>,
    pub port_forwarding: Option<bool>,
    pub random_nat: Option<bool>,
    pub circumvention_routing: Option<bool>,
}
impl Settings {
    pub fn upstream(&self) -> io::Result<LocalAgentSettings> {
        let netshield_level = match self.netshield_level {
            None => None,
            Some(0) => Some(NetshieldLevel::None),
            Some(1) => Some(NetshieldLevel::MalwareFilter),
            Some(2) => Some(NetshieldLevel::AdsAndMalwareFilter),
            Some(3) => Some(NetshieldLevel::AdultAndAdsAndMalwareFilter),
            _ => return Err(failure("netshield_level must be 0, 1, 2 or 3")),
        };
        Ok(LocalAgentSettings {
            split_tcp: self.split_tcp,
            netshield_level,
            soft_jail: self.soft_jail,
            port_forwarding: self.port_forwarding,
            random_nat: self.random_nat,
            circumvention_routing: self.circumvention_routing,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub app_version: String,
    pub login_app_version: String,
    pub cache_dir: String,
    #[serde(default)]
    pub settings: Settings,
    pub certificate_duration_days: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub tun_name: String,
    pub uplink_device: String,
    #[serde(default)]
    pub protocol: ProtocolChoice,
    #[serde(default)]
    pub sni_strategy: SniChoice,
    // The original single-peer fields remain supported for installed r2/r3 configs.
    pub server_ip: Option<IpAddr>,
    pub server_public_key: Option<String>,
    pub private_key_file: Option<String>,
    #[serde(default)]
    pub udp_ports: Vec<u16>,
    #[serde(default)]
    pub tcp_ports: Vec<u16>,
    #[serde(default)]
    pub tls_ports: Vec<u16>,
    pub tls_port: Option<u16>,
    pub server_name: Option<String>,
    pub exit_label: Option<String>,
    #[serde(default)]
    pub peers: Vec<Peer>,
    pub connect_timeout_seconds: u64,
    pub socket_mark: Option<u32>,
    pub local_agent: Option<Agent>,
}

impl Config {
    pub fn load(path: &Path) -> io::Result<(Self, Option<WgClientPrivateKey>)> {
        let config: Self = serde_json::from_slice(&read_file(path, false)?)
            .map_err(|_| failure("Invalid client JSON or unknown configuration field"))?;
        config.validate()?;
        let key = if let Some(path) = &config.private_key_file {
            let bytes = read_file(Path::new(path), true)?;
            let text =
                std::str::from_utf8(&bytes).map_err(|_| failure("Invalid key file encoding"))?;
            Some(WgClientPrivateKey(decode_key(text)?))
        } else {
            None
        };
        Ok((config, key))
    }

    pub fn validate(&self) -> io::Result<()> {
        if !interface_name(&self.tun_name)
            || !interface_name(&self.uplink_device)
            || self.tun_name == self.uplink_device
            || self.uplink_device == "lo"
            || !(1..=600).contains(&self.connect_timeout_seconds)
        {
            return Err(failure("Invalid interface or connection timeout"));
        }
        if let Some(agent) = &self.local_agent {
            if self.private_key_file.is_some() {
                return Err(failure(
                    "Local Agent mode uses its certificate/key cache; remove private_key_file",
                ));
            }
            for version in [&agent.app_version, &agent.login_app_version] {
                let app =
                    muon::App::new(version).map_err(|_| failure("Invalid Proton app version"))?;
                if app.app_version().name().is_none() {
                    return Err(failure(
                        "Use a qualified Proton app version such as platform-product@version",
                    ));
                }
            }
            let app = muon::App::new(&agent.app_version)
                .map_err(|_| failure("Invalid VPN app version"))?;
            if app
                .app_version()
                .name()
                .is_none_or(|n| n.product().to_string() != "vpn")
            {
                return Err(failure(
                    "Local Agent app_version must identify a VPN application",
                ));
            }
            if !Path::new(&agent.cache_dir).is_absolute()
                || agent.certificate_duration_days.is_some_and(|v| v == 0)
            {
                return Err(failure(
                    "Invalid Local Agent cache directory or certificate duration",
                ));
            }
            agent.settings.upstream()?;
        } else if !self
            .private_key_file
            .as_ref()
            .is_some_and(|p| Path::new(p).is_absolute())
        {
            return Err(failure(
                "Private-key mode requires an absolute private_key_file",
            ));
        }
        self.candidates()?;
        Ok(())
    }

    pub fn candidates(&self) -> io::Result<Vec<PeerInfo>> {
        let peers = if self.peers.is_empty() {
            vec![Peer {
                id: "proton".into(),
                server_ip: self.server_ip.ok_or_else(|| failure("Missing server_ip"))?,
                server_public_key: self
                    .server_public_key
                    .clone()
                    .ok_or_else(|| failure("Missing server_public_key"))?,
                udp_ports: self.udp_ports.clone(),
                tcp_ports: self.tcp_ports.clone(),
                tls_ports: if self.tls_ports.is_empty() {
                    self.tls_port.into_iter().collect()
                } else {
                    self.tls_ports.clone()
                },
                priority: 0,
                server_name: self.server_name.clone(),
                exit_label: self.exit_label.clone(),
            }]
        } else {
            if self.server_ip.is_some()
                || self.server_public_key.is_some()
                || self.tls_port.is_some()
                || !self.udp_ports.is_empty()
                || !self.tcp_ports.is_empty()
                || !self.tls_ports.is_empty()
                || self.server_name.is_some()
                || self.exit_label.is_some()
            {
                return Err(failure(
                    "Use either peers or single-peer endpoint fields, not both",
                ));
            }
            self.peers.clone()
        };
        if peers.len() > 32 {
            return Err(failure("At most 32 candidate peers are allowed"));
        }
        let mut ids = std::collections::HashSet::new();
        peers
            .into_iter()
            .map(|p| {
                if p.id.is_empty()
                    || p.id.len() > 64
                    || !ids.insert(p.id.clone())
                    || p.server_ip.is_unspecified()
                    || p.server_ip.is_multicast()
                    || p.server_ip.is_loopback()
                    || p.udp_ports
                        .iter()
                        .chain(&p.tcp_ports)
                        .chain(&p.tls_ports)
                        .any(|&p| p == 0)
                    || p.udp_ports.len() + p.tcp_ports.len() + p.tls_ports.len() > 64
                {
                    return Err(failure("Invalid peer ID, address or protocol ports"));
                }
                if self.local_agent.is_some() && p.server_name.as_ref().is_none_or(|n| n.is_empty())
                {
                    return Err(failure(
                        "Local Agent mode requires server_name for each peer",
                    ));
                }
                let udp_ports = if self.protocol.allows(Protocol::WireguardUdp) {
                    p.udp_ports
                } else {
                    vec![]
                };
                let tcp_ports = if self.protocol.allows(Protocol::WireguardTcp) {
                    p.tcp_ports
                } else {
                    vec![]
                };
                let tls_ports = if self.protocol.allows(Protocol::Stealth) {
                    p.tls_ports
                } else {
                    vec![]
                };
                if udp_ports.is_empty() && tcp_ports.is_empty() && tls_ports.is_empty() {
                    return Err(failure(
                        "Peer has no confirmed ports for the selected protocol",
                    ));
                }
                Ok(PeerInfo {
                    peer_id: p.id,
                    server_ip: IpAddress(p.server_ip),
                    server_public_key: WgPeerPublicKey(decode_key(&p.server_public_key)?),
                    udp_ports,
                    tcp_ports,
                    tls_ports,
                    priority: p.priority,
                    server_name: p.server_name,
                    exit_label: p.exit_label,
                })
            })
            .collect()
    }

    pub fn connection(
        &self,
        private_key: Option<WgClientPrivateKey>,
    ) -> io::Result<InitialConnectionConfig> {
        let connection_mode = if let Some(agent) = &self.local_agent {
            ConnectionMode::LocalAgent {
                user_agent: "protun-openwrt/3.0.5".into(),
                app_version: agent.app_version.clone(),
                settings: agent.settings.upstream()?,
                muon_env: MuonEnv::Prod,
                certificate_duration: agent
                    .certificate_duration_days
                    .map(|count| CertificateDuration::Days { count }),
            }
        } else {
            ConnectionMode::NoLocalAgent {
                wg_private_key: private_key,
            }
        };
        Ok(InitialConnectionConfig {
            peers: self.candidates()?,
            network_available: true,
            pcap_file: None,
            connection_mode,
            sni_strategy: match self.sni_strategy {
                SniChoice::Random => SniStrategy::Random,
                SniChoice::Top => SniStrategy::Top,
            },
        })
    }

    pub fn summary(&self) -> io::Result<String> {
        let p = self.candidates()?;
        Ok(format!(
            "protocol={:?} peers={} udp={} tcp={} tls={} auth={}",
            self.protocol,
            p.len(),
            p.iter().map(|p| p.udp_ports.len()).sum::<usize>(),
            p.iter().map(|p| p.tcp_ports.len()).sum::<usize>(),
            p.iter().map(|p| p.tls_ports.len()).sum::<usize>(),
            if self.local_agent.is_some() {
                "local-agent"
            } else {
                "private-key"
            }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Config {
        serde_json::from_value(serde_json::json!({"tun_name":"protun0","uplink_device":"eth0",
            "server_ip":"192.0.2.1","server_public_key":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
            "private_key_file":"/etc/protun/private.key","tls_port":443,
            "udp_ports":[51820],"tcp_ports":[443],"connect_timeout_seconds":60})).unwrap()
    }
    #[test]
    fn default_remains_stealth_but_all_modes_are_available() {
        let mut c = fixture();
        assert_eq!(c.protocol, ProtocolChoice::Stealth);
        for (mode, expected) in [
            (ProtocolChoice::Stealth, (0, 0, 1)),
            (ProtocolChoice::WireguardUdp, (1, 0, 0)),
            (ProtocolChoice::WireguardTcp, (0, 1, 0)),
            (ProtocolChoice::Auto, (1, 1, 1)),
        ] {
            c.protocol = mode;
            c.validate().unwrap();
            let p = c.candidates().unwrap().remove(0);
            assert_eq!(
                (p.udp_ports.len(), p.tcp_ports.len(), p.tls_ports.len()),
                expected
            );
        }
    }
    #[test]
    fn selected_protocol_without_ports_is_rejected() {
        let mut c = fixture();
        c.protocol = ProtocolChoice::WireguardUdp;
        c.udp_ports.clear();
        assert!(c.validate().is_err());
    }
    #[test]
    fn unknown_protocol_and_settings_are_rejected() {
        assert!(serde_json::from_str::<ProtocolChoice>("\"openvpn\"").is_err());
        assert!(
            Settings {
                netshield_level: Some(4),
                ..Default::default()
            }
            .upstream()
            .is_err()
        );
    }
    #[test]
    fn local_agent_mode_passes_all_settings_to_the_engine() {
        let mut c = fixture();
        c.private_key_file = None;
        c.server_name = Some("test.example".into());
        c.local_agent = Some(Agent {
            app_version: "linux-vpn@3.0.5".into(),
            login_app_version: "linux-vpn@3.0.5".into(),
            cache_dir: "/etc/protun/session".into(),
            certificate_duration_days: Some(7),
            settings: Settings {
                netshield_level: Some(3),
                split_tcp: Some(true),
                soft_jail: Some(false),
                port_forwarding: Some(true),
                random_nat: Some(true),
                circumvention_routing: Some(true),
            },
        });
        c.validate().unwrap();
        let ConnectionMode::LocalAgent {
            settings,
            certificate_duration,
            ..
        } = c.connection(None).unwrap().connection_mode
        else {
            panic!("Local Agent mode was not selected")
        };
        assert_eq!(
            settings.netshield_level,
            Some(NetshieldLevel::AdultAndAdsAndMalwareFilter)
        );
        assert_eq!(
            (
                settings.split_tcp,
                settings.soft_jail,
                settings.port_forwarding,
                settings.random_nat,
                settings.circumvention_routing
            ),
            (Some(true), Some(false), Some(true), Some(true), Some(true))
        );
        assert!(matches!(
            certificate_duration,
            Some(CertificateDuration::Days { count: 7 })
        ));
        c.private_key_file = Some("/etc/protun/private.key".into());
        assert!(c.validate().is_err());
    }
    #[test]
    fn multiple_peers_preserve_priorities_and_reject_duplicate_ids() {
        let mut c = fixture();
        let peer = Peer {
            id: "first".into(),
            server_ip: c.server_ip.take().unwrap(),
            server_public_key: c.server_public_key.take().unwrap(),
            udp_ports: vec![51820],
            tcp_ports: vec![443],
            tls_ports: vec![443],
            priority: 10,
            server_name: None,
            exit_label: None,
        };
        c.udp_ports.clear();
        c.tcp_ports.clear();
        c.tls_port = None;
        c.protocol = ProtocolChoice::Auto;
        let mut second = peer.clone();
        second.id = "second".into();
        second.priority = 20;
        c.peers = vec![peer.clone(), second];
        c.validate().unwrap();
        let peers = c.candidates().unwrap();
        assert_eq!((peers[0].priority, peers[1].priority), (10, 20));
        c.peers.push(peer);
        assert!(c.validate().is_err());
    }
}

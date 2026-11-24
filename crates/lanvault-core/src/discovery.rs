//! Dual-channel LAN peer discovery: mDNS-SD (`_lanvault._tcp.local.`) + UDP broadcast fallback.
//!
//! Reference implementation: `vendor_refs/rust-mdns-sd/examples/{register.rs, query.rs}`
//! and `vendor_refs/rust-mdns-sd/src/service_info.rs`.
//! TXT records carry `id`, `os`, `name`, and `fp` (Ed25519 public key fingerprint).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::error::{LanVaultError, Result};

/// Canonical mDNS service type for LanVault nodes.
pub const MDNS_SERVICE_TYPE: &str = "_lanvault._tcp.local.";
/// UDP broadcast fallback port when multicast mDNS is blocked by router AP isolation.
pub const UDP_FALLBACK_PORT: u16 = 7843;

/// Local node metadata advertised over mDNS TXT records and UDP broadcast beacons.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryAnnouncement {
    pub id: String,
    pub os: String,
    pub name: String,
    pub fp: String,
    pub port: u16,
}

/// Discovered peer on the local network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredPeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub os_type: String,
    pub fingerprint: String,
    pub ip: IpAddr,
    pub port: u16,
    pub via_mdns: bool,
}

/// Active mDNS registration handle. Call `shutdown()` on graceful exit.
pub struct MdnsRegistrar {
    daemon: Option<ServiceDaemon>,
    fullname: Option<String>,
    udp_beacon_task: Option<JoinHandle<()>>,
}

impl MdnsRegistrar {
    /// Registers this node on `_lanvault._tcp.local.` with TXT keys `id`, `os`, `name`, `fp`
    /// and starts a periodic `255.255.255.255` UDP broadcast fallback beacon.
    pub fn register(announcement: DiscoveryAnnouncement) -> Result<Self> {
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.discovery",
            "discovery.mdns_register",
            Some(format!("id={} port={}", announcement.id, announcement.port)),
        );
        let udp_beacon_task = Some(spawn_udp_beacon_sender(announcement.clone()));
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(err) => {
                tracing::warn!("mDNS registration unavailable; UDP beacon remains active: {err}");
                return Ok(Self {
                    daemon: None,
                    fullname: None,
                    udp_beacon_task,
                });
            }
        };

        let hostname = format!("lanvault-{}.local.", sanitize_label(&announcement.id));
        let properties = [
            ("id", announcement.id.as_str()),
            ("os", announcement.os.as_str()),
            ("name", announcement.name.as_str()),
            ("fp", announcement.fp.as_str()),
        ];

        let service_info = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &announcement.id,
            &hostname,
            "",
            announcement.port,
            &properties[..],
        )
        .map_err(|e| LanVaultError::Discovery(format!("invalid mDNS ServiceInfo: {e}")))?
        .enable_addr_auto();

        let fullname = service_info.get_fullname().to_string();
        if let Err(err) = daemon.register(service_info) {
            tracing::warn!("mDNS registration failed; UDP beacon remains active: {err}");
            let _ = daemon.shutdown();
            return Ok(Self {
                daemon: None,
                fullname: None,
                udp_beacon_task,
            });
        }

        Ok(Self {
            daemon: Some(daemon),
            fullname: Some(fullname),
            udp_beacon_task,
        })
    }

    /// Unregisters the mDNS service and stops the UDP fallback beacon task.
    pub fn shutdown(mut self) -> Result<()> {
        if let Some(handle) = self.udp_beacon_task.take() {
            handle.abort();
        }
        if let (Some(daemon), Some(fullname)) = (self.daemon.take(), self.fullname.take()) {
            let _ = daemon.unregister(&fullname);
            daemon
                .shutdown()
                .map_err(|e| LanVaultError::Discovery(format!("mDNS shutdown failed: {e}")))?;
        }
        Ok(())
    }
}

impl Drop for MdnsRegistrar {
    fn drop(&mut self) {
        if let Some(handle) = self.udp_beacon_task.take() {
            handle.abort();
        }
        if let Some(daemon) = self.daemon.take() {
            if let Some(fullname) = self.fullname.take() {
                let _ = daemon.unregister(&fullname);
            }
            let _ = daemon.shutdown();
        }
    }
}

/// Active dual-channel browser (mDNS + UDP broadcast listener).
pub struct DiscoveryBrowser {
    daemon: Option<ServiceDaemon>,
    tasks: Vec<JoinHandle<()>>,
}

impl DiscoveryBrowser {
    /// Starts browsing `_lanvault._tcp.local.` and listening on UDP `UDP_FALLBACK_PORT`,
    /// forwarding discovered peers into `tx`. Own `local_device_id` is filtered out.
    pub fn start(local_device_id: String, tx: mpsc::Sender<DiscoveredPeerInfo>) -> Result<Self> {
        let _probe = crate::debug_telemetry::ProbeGuard::start(
            "core.discovery",
            "discovery.browser_start",
            Some(format!("local_id={local_device_id}")),
        );
        let daemon = ServiceDaemon::new().ok();
        let receiver = daemon
            .as_ref()
            .and_then(|daemon| daemon.browse(MDNS_SERVICE_TYPE).ok());
        let mut tasks = Vec::new();
        if let Some(receiver) = receiver {
            let mdns_tx = tx.clone();
            let mdns_local_id = local_device_id.clone();
            tasks.push(tokio::spawn(async move {
                while let Ok(event) = receiver.recv_async().await {
                    if let ServiceEvent::ServiceResolved(info) = event {
                        let Some(id) = info.get_property_val_str("id") else {
                            continue;
                        };
                        if id == mdns_local_id {
                            continue;
                        }
                        let Some(os) = info.get_property_val_str("os") else {
                            continue;
                        };
                        let Some(name) = info.get_property_val_str("name") else {
                            continue;
                        };
                        let Some(fp) = info.get_property_val_str("fp") else {
                            continue;
                        };
                        let Some(ipv4) = info.get_addresses_v4().into_iter().next() else {
                            continue;
                        };
                        let peer = DiscoveredPeerInfo {
                            device_id: id.to_string(),
                            device_name: name.to_string(),
                            os_type: os.to_string(),
                            fingerprint: fp.to_string(),
                            ip: IpAddr::V4(ipv4),
                            port: info.get_port(),
                            via_mdns: true,
                        };
                        crate::debug_telemetry::record_debug(
                            "core.discovery",
                            Some("discovery.peer_found"),
                            None,
                            || {
                                format!(
                                    "mDNS resolved peer {} ({}:{})",
                                    peer.device_id, peer.ip, peer.port
                                )
                            },
                            Some(format!("os={} name={}", peer.os_type, peer.device_name)),
                        );
                        if mdns_tx.send(peer).await.is_err() {
                            break;
                        }
                    }
                }
            }));
        } else {
            tracing::warn!("mDNS discovery unavailable; continuing with UDP fallback");
        }

        let udp_task = spawn_udp_beacon_listener(local_device_id, tx);
        tasks.push(udp_task);

        Ok(Self { daemon, tasks })
    }

    /// Stops browsing and shuts down background listeners.
    pub fn shutdown(mut self) -> Result<()> {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(daemon) = self.daemon.take() {
            daemon.shutdown().map_err(|e| {
                LanVaultError::Discovery(format!("mDNS browser shutdown failed: {e}"))
            })?;
        }
        Ok(())
    }
}

impl Drop for DiscoveryBrowser {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.shutdown();
        }
    }
}

fn spawn_udp_beacon_sender(announcement: DiscoveryAnnouncement) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Ok(socket) = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await else {
            warn!("UDP broadcast sender failed to bind ephemeral socket");
            return;
        };
        if let Err(e) = socket.set_broadcast(true) {
            warn!("failed to enable SO_BROADCAST on discovery socket: {e}");
            return;
        }
        let Ok(payload) = serde_json::to_vec(&announcement) else {
            return;
        };
        let target = SocketAddr::from((Ipv4Addr::BROADCAST, UDP_FALLBACK_PORT));
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(e) = socket.send_to(&payload, target).await {
                debug!("UDP broadcast beacon send skipped: {e}");
            }
        }
    })
}

fn spawn_udp_beacon_listener(
    local_device_id: String,
    tx: mpsc::Sender<DiscoveredPeerInfo>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, UDP_FALLBACK_PORT));
        let Ok(socket) = UdpSocket::bind(bind_addr).await else {
            warn!("UDP fallback listener could not bind port {UDP_FALLBACK_PORT}");
            return;
        };
        let mut buf = [0u8; 2048];
        loop {
            let Ok((len, src)) = socket.recv_from(&mut buf).await else {
                break;
            };
            let Ok(beacon) = serde_json::from_slice::<DiscoveryAnnouncement>(&buf[..len]) else {
                continue;
            };
            if beacon.id == local_device_id {
                continue;
            }
            let peer = DiscoveredPeerInfo {
                device_id: beacon.id,
                device_name: beacon.name,
                os_type: beacon.os,
                fingerprint: beacon.fp,
                ip: src.ip(),
                port: beacon.port,
                via_mdns: false,
            };
            crate::debug_telemetry::record_debug(
                "core.discovery",
                Some("discovery.peer_found"),
                None,
                || {
                    format!(
                        "UDP beacon discovered peer {} ({}:{})",
                        peer.device_id, peer.ip, peer.port
                    )
                },
                Some(format!("os={} name={}", peer.os_type, peer.device_name)),
            );
            if tx.send(peer).await.is_err() {
                break;
            }
        }
    })
}

fn sanitize_label(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "node".to_string()
    } else {
        cleaned
    }
}

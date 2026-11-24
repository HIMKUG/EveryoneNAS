//! TLS for LAN traffic, with legacy HTTP restricted to the host's loopback interface.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use lanvault_core::{DeviceIdentity, LanVaultError, Result};
use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, PKCS_ED25519};
use rustls::pki_types::PrivatePkcs8KeyDer;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_HANDSHAKES: usize = 64;

pub(crate) trait ConnectionIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ConnectionIo for T {}
type Connection = (Box<dyn ConnectionIo>, LanConnectionInfo);

#[derive(Clone, Copy, Debug)]
pub(crate) struct LanConnectionInfo {
    pub(crate) addr: SocketAddr,
    pub(crate) secure: bool,
}

impl Connected<IncomingStream<'_, LanListener>> for LanConnectionInfo {
    fn connect_info(stream: IncomingStream<'_, LanListener>) -> Self {
        *stream.remote_addr()
    }
}

pub(crate) struct LanListener {
    tcp: TcpListener,
    tls: TlsAcceptor,
    pending: JoinSet<Option<Connection>>,
}

impl LanListener {
    pub(crate) fn new(tcp: TcpListener, identity: &DeviceIdentity, lan_ip: IpAddr) -> Result<Self> {
        let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
        if !lan_ip.is_unspecified() && !lan_ip.is_loopback() {
            names.push(lan_ip.to_string());
        }
        let binding = tcp.local_addr()?.ip();
        if !binding.is_unspecified() && !binding.is_loopback() && binding != lan_ip {
            names.push(binding.to_string());
        }
        let mut params = CertificateParams::new(names)
            .map_err(|error| LanVaultError::Crypto(error.to_string()))?;
        params.distinguished_name.push(DnType::CommonName, format!("EveryoneNAS {}", identity.fingerprint_hex()));
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let encoded = identity.tls_key_pkcs8()?;
        let encoded_key = PrivatePkcs8KeyDer::from(encoded.as_slice());
        let signer = KeyPair::from_pkcs8_der_and_sign_algo(&encoded_key, &PKCS_ED25519)
            .map_err(|error| LanVaultError::Crypto(error.to_string()))?;
        let certificate = params.self_signed(&signer)
            .map_err(|error| LanVaultError::Crypto(error.to_string()))?;
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .and_then(|builder| builder.with_no_client_auth().with_single_cert(
                vec![certificate.der().clone()],
                PrivatePkcs8KeyDer::from(signer.serialize_der()).into(),
            ))
            .map_err(|error| LanVaultError::Crypto(error.to_string()))?;
        Ok(Self { tcp, tls: TlsAcceptor::from(Arc::new(config)), pending: JoinSet::new() })
    }
}

impl Listener for LanListener {
    type Io = Box<dyn ConnectionIo>;
    type Addr = LanConnectionInfo;

    async fn accept(&mut self) -> Connection {
        loop {
            tokio::select! {
                result = self.pending.join_next(), if !self.pending.is_empty() => {
                    if let Some(Ok(Some(connection))) = result {
                        return connection;
                    }
                }
                result = self.tcp.accept(), if self.pending.len() < MAX_PENDING_HANDSHAKES => {
                    match result {
                        Ok((stream, addr)) => {
                            let tls = self.tls.clone();
                            self.pending.spawn(async move {
                                tokio::time::timeout(HANDSHAKE_TIMEOUT, negotiate(stream, addr, tls))
                                    .await.ok()?.ok()
                            });
                        }
                        Err(error) => {
                            tracing::warn!(%error, "LAN TCP accept failed");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<LanConnectionInfo> {
        self.tcp.local_addr().map(|addr| LanConnectionInfo { addr, secure: true })
    }
}

async fn negotiate(stream: TcpStream, addr: SocketAddr, tls: TlsAcceptor) -> io::Result<Connection> {
    if addr.ip().to_canonical().is_loopback() {
        let mut first = [0];
        if stream.peek(&mut first).await? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if first[0] != 0x16 {
            return Ok((Box::new(stream), LanConnectionInfo { addr, secure: false }));
        }
    }
    tls.accept(stream).await.map(|stream| {
        (Box::new(stream) as Box<dyn ConnectionIo>, LanConnectionInfo { addr, secure: true })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::WebSocketUpgrade;
    use axum::{routing::get, Router};
    use lanvault_client::transport::{peer_http_builder, PeerFingerprint};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn lan_https_verifies_device_identity_and_supports_wss() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind test listener");
        let addr = tcp.local_addr().expect("bound address");
        let identity = DeviceIdentity::generate();
        let listener = LanListener::new(tcp, &identity, addr.ip()).expect("test certificate");
        let app = Router::new()
            .route("/", get(|| async { "EveryoneNAS" }))
            .route("/ws", get(|upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(|_| async {})
            }))
            .route("/transport", get(|axum::extract::ConnectInfo(info): axum::extract::ConnectInfo<LanConnectionInfo>| async move {
                if info.secure { "secure" } else { "loopback" }
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<LanConnectionInfo>())
                .await.expect("serve");
        });
        let pin: PeerFingerprint = identity.fingerprint_hex().parse().expect("device fingerprint");
        let client = peer_http_builder(pin).expect("client").build().expect("HTTPS client");
        let base = format!("https://{addr}");
        let body = client.get(format!("{base}/transport")).send().await.expect("pinned TLS")
            .text().await.expect("response body");
        assert_eq!(body, "secure");

        let wrong_pin = PeerFingerprint::from_public_key(&DeviceIdentity::generate().public_key_bytes());
        let wrong = peer_http_builder(wrong_pin).expect("client").build().expect("wrong pin client");
        assert!(wrong.get(&base).send().await.is_err());
        assert!(client.get(format!("http://{addr}/")).send().await.is_err());

        let upgraded = client.get(format!("{base}/ws"))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", "A".repeat(22) + "==")
            .send().await.expect("WSS response");
        assert_eq!(upgraded.status().as_u16(), 101);

        let mut local = TcpStream::connect(addr).await.expect("loopback socket");
        local.write_all(b"GET /transport HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await.expect("loopback request");
        let mut response = String::new();
        local.read_to_string(&mut response).await.expect("loopback response");
        assert!(response.ends_with("loopback"));
        server.abort();
    }

    #[tokio::test]
    async fn non_loopback_connection_cannot_negotiate_plain_http() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind test listener");
        let addr = tcp.local_addr().expect("bound address");
        let listener = LanListener::new(tcp, &DeviceIdentity::generate(), addr.ip()).expect("test certificate");
        let mut client = TcpStream::connect(addr).await.expect("test client");
        let (stream, _) = listener.tcp.accept().await.expect("test connection");
        client.write_all(b"GET / HTTP/1.1\r\nHost: example\r\n\r\n").await.expect("test request");
        let remote = SocketAddr::from(([192, 0, 2, 1], 12345));
        assert!(negotiate(stream, remote, listener.tls.clone()).await.is_err());
    }
}

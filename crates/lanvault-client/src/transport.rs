//! LAN HTTPS clients authenticate a device key entered during pairing or saved in a trusted peer.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{redirect::Policy, Client, ClientBuilder};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, Error, SignatureScheme};

use crate::ClientError;

// RFC 8410 Ed25519 SubjectPublicKeyInfo with an absent parameters field and a 32-byte key.
const ED25519_SPKI_PREFIX: &[u8] = &[
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerFingerprint([u8; 16]);

impl PeerFingerprint {
    pub fn from_public_key(key: &[u8; 32]) -> Self {
        Self(lanvault_core::crypto::public_key_fingerprint_bytes(key))
    }
}

impl FromStr for PeerFingerprint {
    type Err = ClientError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut pin = [0; 16];
        hex::decode_to_slice(value.trim(), &mut pin).map_err(|_| {
            ClientError::InvalidInput("peer fingerprint must be 32 hexadecimal characters".into())
        })?;
        Ok(Self(pin))
    }
}

pub fn peer_http_builder(pin: PeerFingerprint) -> Result<ClientBuilder, ClientError> {
    let provider = rustls::crypto::ring::default_provider();
    let verifier = DeviceCertificateVerifier {
        pin,
        algorithms: provider.signature_verification_algorithms,
    };
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| ClientError::Crypto(error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Client::builder()
        .use_preconfigured_tls(config)
        .https_only(true)
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(5)))
}

#[derive(Debug)]
struct DeviceCertificateVerifier {
    pin: PeerFingerprint,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for DeviceCertificateVerifier {
    fn verify_server_cert(
        &self,
        certificate: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let parsed = ParsedCertificate::try_from(certificate)?;
        let spki = parsed.subject_public_key_info();
        let key: &[u8; 32] = spki
            .as_ref()
            .strip_prefix(ED25519_SPKI_PREFIX)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if PeerFingerprint::from_public_key(key) != self.pin {
            return Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        // Connection addresses can change; the pinned device key is the trust anchor.
        // rustls still verifies possession of the private key in CertificateVerify below.
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, certificate, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, certificate, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

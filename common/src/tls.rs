//! TLS for every connection: the auth service, chat servers and the client.
//!
//! Each server and the auth service makes its own self-signed certificate the first time it
//! runs. Clients use trust-on-first-use: the first certificate seen from an address is
//! remembered (its fingerprint), and a different certificate from that address later is refused.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream as AsyncTcpStream;
use tokio_rustls::TlsAcceptor;

/// A blocking TLS connection, as used by the client.
pub type Stream = StreamOwned<ClientConnection, std::net::TcpStream>;

pub use tokio_rustls::client::TlsStream as AsyncClientStream;
pub use tokio_rustls::server::TlsStream as AsyncServerStream;

/// The name a certificate is issued for. Clients don't check names; they pin the certificate.
const CERT_NAME: &str = "disscord";

/// Makes a new self-signed certificate. Returns (certificate PEM, private key PEM).
pub fn new_identity() -> anyhow::Result<(String, String)> {
    let certified = rcgen::generate_simple_self_signed(vec![CERT_NAME.to_string(), "localhost".to_string()])?;
    Ok((certified.cert.pem(), certified.key_pair.serialize_pem()))
}

/// The acceptor for incoming TLS connections, built from a PEM certificate and key.
pub fn acceptor(cert_pem: &str, key_pem: &str) -> anyhow::Result<TlsAcceptor> {
    use rustls::pki_types::pem::PemObject;
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// SHA-256 of a certificate, written as hex. This is what clients remember.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    Sha256::digest(cert.as_ref()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Decides what to do with a server's certificate, given the fingerprint remembered for it.
/// Returns `Ok(true)` when this is the first time, so the fingerprint should be remembered.
pub fn check_pin(remembered: Option<&str>, observed: &str) -> Result<bool, String> {
    match remembered {
        None => Ok(true),
        Some(known) if known == observed => Ok(false),
        Some(_) => Err(
            "This server's certificate has changed since you last connected. \
             If you trust the change, choose Trust the new certificate."
                .to_string(),
        ),
    }
}

/// Accepts any certificate, but records its fingerprint so the caller can pin it.
#[derive(Debug)]
struct Tofu {
    seen: Arc<Mutex<Option<String>>>,
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Tofu {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap() = Some(fingerprint(end_entity));
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// A client configuration that records the server's certificate fingerprint in `seen`.
fn client_config(seen: Arc<Mutex<Option<String>>>) -> anyhow::Result<Arc<ClientConfig>> {
    let provider = CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    let verifier = Tofu { seen, algorithms: provider.signature_verification_algorithms };
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Connects over TLS without blocking the caller for long. Returns the stream and the
/// server's certificate fingerprint. The caller must check it with `check_pin` before sending
/// anything private.
pub fn connect_blocking(
    addr: &str,
    timeout: Duration,
) -> std::io::Result<(StreamOwned<ClientConnection, std::net::TcpStream>, String)> {
    use std::io::{Error, ErrorKind};
    use std::net::ToSocketAddrs;
    let target = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "address did not resolve"))?;
    let mut tcp = std::net::TcpStream::connect_timeout(&target, timeout)?;
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(timeout))?;

    let seen = Arc::new(Mutex::new(None));
    let config = client_config(seen.clone()).map_err(Error::other)?;
    let name = ServerName::try_from(CERT_NAME).map_err(Error::other)?;
    let mut conn = ClientConnection::new(config, name).map_err(Error::other)?;
    conn.complete_io(&mut tcp)
        .map_err(|e| Error::other(format!("secure connection failed: {e}")))?;
    let fingerprint = seen
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| Error::other("no certificate from server"))?;
    Ok((StreamOwned::new(conn, tcp), fingerprint))
}

/// The async version of `connect_blocking`, for the chat server talking to the auth service
/// and to other servers.
pub async fn connect_async(addr: &str) -> anyhow::Result<(AsyncClientStream<AsyncTcpStream>, String)> {
    let seen = Arc::new(Mutex::new(None));
    let config = client_config(seen.clone())?;
    let connector = tokio_rustls::TlsConnector::from(config);
    let tcp = AsyncTcpStream::connect(addr).await?;
    let name = ServerName::try_from(CERT_NAME)?;
    let stream = connector.connect(name, tcp).await?;
    let fingerprint = seen.lock().unwrap().clone().ok_or_else(|| anyhow::anyhow!("no certificate from server"))?;
    Ok((stream, fingerprint))
}

/// True when a connection ended because the other side dropped the socket without the TLS close
/// message. That happens when a client exits, so it is not worth logging as an error.
pub fn is_quiet_close(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)
}

//! TLS for the HTTPS port (autocert's `TLSConfig`), and the HTTP port's handler when TLS is on
//! (`manager.HTTPHandler(httpRedirectHandler)`): ACME HTTP-01 responses, and a permanent
//! redirect to HTTPS for everything else.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use rustls::server::{Acceptor, ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ServerConfig, crypto::ring};
use tokio::net::TcpStream;
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;

use super::acme::{CertManager, Error, to_ascii};
use super::conn::Protocol;

const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

pub struct Tls {
    certs: Arc<CertManager>,
    config: Arc<ServerConfig>,
    challenge_config: Arc<ServerConfig>,
}

impl Tls {
    pub fn new(certs: Arc<CertManager>) -> Result<Self, Error> {
        let server_config = |resolver: Arc<dyn ResolvesServerCert>, alpn: &[&[u8]]| -> Result<ServerConfig, Error> {
            let mut config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_no_client_auth()
                .with_cert_resolver(resolver);
            config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
            Ok(config)
        };
        let mut config = server_config(Arc::new(Loaded(certs.clone())), &[b"h2", b"http/1.1"])?;
        // Session tickets, as Go's crypto/tls issues by default and Thruster left on, so returning
        // clients resume beyond rustls's 256-entry session cache. The keys live in memory and
        // rotate every 6 hours. (A challenge handshake ends there, so its config has none.)
        config.ticketer = ring::Ticketer::new()?;
        Ok(Self {
            config: Arc::new(config),
            challenge_config: Arc::new(server_config(Arc::new(Challenges(certs.clone())), &[ACME_TLS_ALPN])?),
            certs,
        })
    }

    /// Completes a handshake, obtaining the certificate first if need be. `None` for a TLS-ALPN-01
    /// validation, which ends with the handshake.
    pub async fn accept(&self, stream: TcpStream) -> Result<Option<(TlsStream<TcpStream>, Protocol)>, Error> {
        let start = LazyConfigAcceptor::new(Acceptor::default(), stream).await?;
        let hello = start.client_hello();
        let server_name = hello.server_name().map(str::to_string);
        if hello.alpn().is_some_and(|mut protocols| protocols.any(|p| p == ACME_TLS_ALPN)) {
            let _ = start.into_stream(self.challenge_config.clone()).await?;
            return Ok(None);
        }
        self.certs.certificate(server_name.as_deref()).await?;
        let stream = start.into_stream(self.config.clone()).await?;
        let protocol = if stream.get_ref().1.alpn_protocol() == Some(b"h2") { Protocol::Http2 } else { Protocol::Http1 };
        Ok(Some((stream, protocol)))
    }
}

/// Serves the certificates `CertManager` has loaded.
#[derive(Debug)]
struct Loaded(Arc<CertManager>);

impl ResolvesServerCert for Loaded {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = to_ascii(hello.server_name()?.trim_end_matches('.'))?;
        self.0.loaded(&name)
    }
}

/// Serves TLS-ALPN-01 challenge certificates.
#[derive(Debug)]
struct Challenges(Arc<CertManager>);

impl ResolvesServerCert for Challenges {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0.challenge_certificate(hello.server_name())
    }
}

impl std::fmt::Debug for CertManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertManager").finish_non_exhaustive()
    }
}

/// The HTTP port's handler with TLS on.
pub fn http_handler(certs: &CertManager, request: &Request<Body>) -> Response<Body> {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default();
    let path = request.uri().path();
    if path.starts_with("/.well-known/acme-challenge/") {
        if !certs.host_allowed(&host) {
            return error(StatusCode::FORBIDDEN, &format!("acme/autocert: host {host:?} not configured in HostWhitelist"));
        }
        return match certs.http_token(path) {
            Some(token) => {
                let mut response = Response::new(Body::from(token));
                response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
                response
            }
            None => error(StatusCode::NOT_FOUND, "acme/autocert: certificate cache miss"),
        };
    }
    redirect(certs, request, &host)
}

/// `httpRedirectHandler`: 301 to the same path over HTTPS for TLS_DOMAIN hosts, 421 for others.
fn redirect(certs: &CertManager, request: &Request<Body>, host: &str) -> Response<Body> {
    let host = split_host_port(host);
    let mut response = match to_ascii(host).filter(|host| certs.host_allowed(host)) {
        None => error(StatusCode::MISDIRECTED_REQUEST, "Misdirected Request"),
        Some(host) => {
            let request_uri = request.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
            let url = format!("https://{host}{request_uri}");
            let body = if request.method() == Method::GET {
                Body::from(format!("<a href=\"{}\">Moved Permanently</a>.\n\n", html_escape(&url)))
            } else {
                Body::empty()
            };
            let mut response = Response::new(body);
            *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
            if let Ok(location) = HeaderValue::from_str(&url) {
                response.headers_mut().insert(header::LOCATION, location);
            }
            if request.method() == Method::GET || request.method() == Method::HEAD {
                response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
            }
            response
        }
    };
    response.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}

/// `net.SplitHostPort`, or the host as given.
fn split_host_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return match rest.split_once("]:") {
            Some((address, _)) => address,
            None => host,
        };
    }
    match host.rsplit_once(':') {
        Some((name, _)) if !name.contains(':') => name,
        _ => host,
    }
}

/// `http.Error`
fn error(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(Body::from(format!("{message}\n")));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;").replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::acme::AcmeOptions;
    use http_body_util::BodyExt;
    use rustls::HandshakeKind;
    use rustls::pki_types::{CertificateDer, ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn certs() -> Arc<CertManager> {
        certs_in(std::env::temp_dir().join("matchbox-front-tls-test"))
    }

    fn certs_in(storage_path: std::path::PathBuf) -> Arc<CertManager> {
        CertManager::new(AcmeOptions {
            directory_url: "https://acme.invalid/directory".into(),
            external_account: None,
            storage_path,
            domains: vec!["chat.example.com".into()],
            challenge_types: vec![],
            directory_root: None,
        })
    }

    #[tokio::test]
    async fn returning_clients_resume_their_sessions_by_ticket() {
        let storage = tempfile::tempdir().unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let certificate = rcgen::CertificateParams::new(vec!["chat.example.com".to_string()]).unwrap().self_signed(&key).unwrap();
        std::fs::write(storage.path().join("chat.example.com"), key.serialize_pem() + &certificate.pem()).unwrap();
        let tls = Tls::new(certs_in(storage.path().to_path_buf())).unwrap();
        assert!(!tls.challenge_config.ticketer.enabled());
        let port = serve(tls).await;

        let returning = client(certificate.der());
        assert_eq!(handshake(port, &returning).await, HandshakeKind::Full);
        // At two tickets a session, these push the first one out of rustls's 256-entry session
        // cache, which is all a returning client could resume from without tickets.
        for _ in 0..150 {
            handshake(port, &client(certificate.der())).await;
        }
        assert_eq!(handshake(port, &returning).await, HandshakeKind::Resumed);
    }

    /// Serves TLS on a local port, closing each connection once its handshake is done.
    async fn serve(tls: Tls) -> u16 {
        let tls = Arc::new(tls);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let tls = tls.clone();
                tokio::spawn(async move {
                    if let Ok(Some((mut stream, _))) = tls.accept(stream).await {
                        let _ = stream.shutdown().await;
                    }
                });
            }
        });
        port
    }

    /// A client that trusts `certificate` and remembers its sessions.
    fn client(certificate: &CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate.clone()).unwrap();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    }

    async fn handshake(port: u16, config: &Arc<rustls::ClientConfig>) -> HandshakeKind {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let name = ServerName::try_from("chat.example.com").unwrap();
        let mut stream = tokio_rustls::TlsConnector::from(config.clone()).connect(name, stream).await.unwrap();
        // Reading to the end takes in the tickets the server sends after the handshake.
        stream.read_to_end(&mut Vec::new()).await.unwrap();
        stream.get_ref().1.handshake_kind().unwrap()
    }

    async fn get(method: Method, uri: &str, host: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        let request = Request::builder().method(method).uri(uri).header("host", host).body(Body::empty()).unwrap();
        let response = http_handler(&certs(), &request);
        let (parts, body) = response.into_parts();
        (parts.status, parts.headers, String::from_utf8(body.collect().await.unwrap().to_bytes().to_vec()).unwrap())
    }

    #[tokio::test]
    async fn redirects_configured_hosts_to_https() {
        let (status, headers, body) = get(Method::GET, "/rooms/1?x=1&y=%3C", "Chat.Example.com:80").await;
        assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
        assert_eq!(headers[header::LOCATION], "https://chat.example.com/rooms/1?x=1&y=%3C");
        assert_eq!(headers[header::CONNECTION], "close");
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(body, "<a href=\"https://chat.example.com/rooms/1?x=1&amp;y=%3C\">Moved Permanently</a>.\n\n");

        let (status, headers, body) = get(Method::POST, "/session", "chat.example.com").await;
        assert_eq!(status, StatusCode::MOVED_PERMANENTLY);
        assert!(!headers.contains_key(header::CONTENT_TYPE));
        assert_eq!(body, "");
    }

    #[tokio::test]
    async fn refuses_other_hosts() {
        let (status, _, body) = get(Method::GET, "/", "evil.example.com").await;
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
        assert_eq!(body, "Misdirected Request\n");
        let (status, _, _) = get(Method::GET, "/", "[::1]:80").await;
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn answers_http01_challenges() {
        let (status, _, body) = get(Method::GET, "/.well-known/acme-challenge/abc", "chat.example.com").await;
        assert_eq!((status, body.as_str()), (StatusCode::NOT_FOUND, "acme/autocert: certificate cache miss\n"));
        let (status, _, _) = get(Method::GET, "/.well-known/acme-challenge/abc", "chat.example.com:80").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

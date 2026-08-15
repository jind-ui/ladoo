//! TLS support via rustls.
//!
//! Enabled by the `tls` feature flag. Provides [`TlsConfig`] for
//! loading PEM certificate chains and private keys, building a
//! [`rustls::ServerConfig`] with ALPN `["h2", "http/1.1"]`.

use std::path::PathBuf;
use std::sync::Arc;

/// TLS configuration holding paths to the certificate chain and
/// private key in PEM format.
pub(crate) struct TlsConfig {
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl TlsConfig {
    /// Create a new TLS configuration with the given certificate and
    /// key file paths.
    pub(crate) fn new(cert_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>) -> Self {
        Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
        }
    }

    /// Load the PEM files and build a `TlsAcceptor`.
    ///
    /// # Panics
    ///
    /// Panics if the certificate or key files cannot be read or parsed.
    /// A misconfigured server should never accept traffic.
    pub(crate) fn build_acceptor(&self) -> tokio_rustls::TlsAcceptor {
        use rustls_pemfile::{certs, private_key};

        // rustls 0.23 requires a process-wide crypto provider to be
        // installed before any `ServerConfig` can be built. Installing
        // is a one-time, process-global operation — later calls (e.g.
        // from other `TlsConfig`s, or concurrent tests) return `Err`
        // with the already-installed provider, which is fine to ignore.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let cert_file = std::fs::File::open(&self.cert_path).unwrap_or_else(|e| {
            panic!(
                "TLS certificate file {:?} could not be opened: {e}",
                self.cert_path
            )
        });
        let key_file = std::fs::File::open(&self.key_path).unwrap_or_else(|e| {
            panic!(
                "TLS private key file {:?} could not be opened: {e}",
                self.key_path
            )
        });

        let certs: Vec<_> = certs(&mut std::io::BufReader::new(cert_file))
            .collect::<Result<Vec<_>, _>>()
            .expect("failed to parse TLS certificate PEM");

        if certs.is_empty() {
            panic!(
                "TLS certificate file {:?} contains no certificates",
                self.cert_path
            );
        }

        let key = private_key(&mut std::io::BufReader::new(key_file))
            .expect("failed to parse TLS private key PEM")
            .unwrap_or_else(|| {
                panic!(
                    "TLS private key file {:?} contains no private key",
                    self.key_path
                )
            });

        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("invalid TLS certificate/key pair");

        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        tokio_rustls::TlsAcceptor::from(Arc::new(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::request::Request;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn build_acceptor_from_valid_pem() {
        let config = TlsConfig::new(fixture_path("cert.pem"), fixture_path("key.pem"));
        let _acceptor = config.build_acceptor();
    }

    #[test]
    #[should_panic(expected = "TLS certificate")]
    fn panics_on_missing_cert() {
        let config = TlsConfig::new("/nonexistent/cert.pem", fixture_path("key.pem"));
        config.build_acceptor();
    }

    #[test]
    #[should_panic(expected = "TLS private key")]
    fn panics_on_missing_key() {
        let config = TlsConfig::new(fixture_path("cert.pem"), "/nonexistent/key.pem");
        config.build_acceptor();
    }

    #[test]
    #[should_panic]
    fn panics_on_malformed_pem() {
        // Write garbage to a temp file and try to use it as a cert.
        let dir = tempfile::tempdir().unwrap();
        let bad_cert = dir.path().join("bad.pem");
        std::fs::write(&bad_cert, b"not a real certificate").unwrap();
        let config = TlsConfig::new(bad_cert, fixture_path("key.pem"));
        config.build_acceptor();
    }

    #[tokio::test]
    async fn tls_serves_https() {
        let tls_config = TlsConfig::new(fixture_path("cert.pem"), fixture_path("key.pem"));
        let acceptor = tls_config.build_acceptor();

        let app = App::new().get("/secure", |_: Request| "tls works");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, _tls) =
            app.into_parts();

        let handle = tokio::spawn(async move {
            crate::server::serve(
                router,
                listener,
                Arc::new(state),
                middleware,
                std::future::pending::<()>(),
                shutdown_timeout,
                shutdown_hooks,
                body_limit,
                Some(acceptor),
            )
            .await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let cert_pem = std::fs::read(fixture_path("cert.pem")).unwrap();
        let root_cert = reqwest::Certificate::from_pem(&cert_pem).unwrap();

        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(root_cert)
            .build()
            .unwrap();

        let resp = client
            .get(format!("https://localhost:{}/secure", addr.port()))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.version(), reqwest::Version::HTTP_2);
        assert_eq!(resp.text().await.unwrap(), "tls works");

        handle.abort();
    }

    #[tokio::test]
    async fn tls_also_serves_http1() {
        let tls_config = TlsConfig::new(fixture_path("cert.pem"), fixture_path("key.pem"));
        let acceptor = tls_config.build_acceptor();

        let app = App::new().get("/hello", |_: Request| "http1 tls");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (router, state, middleware, shutdown_timeout, shutdown_hooks, body_limit, _tls) =
            app.into_parts();

        let handle = tokio::spawn(async move {
            crate::server::serve(
                router,
                listener,
                Arc::new(state),
                middleware,
                std::future::pending::<()>(),
                shutdown_timeout,
                shutdown_hooks,
                body_limit,
                Some(acceptor),
            )
            .await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let cert_pem = std::fs::read(fixture_path("cert.pem")).unwrap();
        let root_cert = reqwest::Certificate::from_pem(&cert_pem).unwrap();

        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(root_cert)
            .http1_only()
            .build()
            .unwrap();

        let resp = client
            .get(format!("https://localhost:{}/hello", addr.port()))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        assert_eq!(resp.version(), reqwest::Version::HTTP_11);
        assert_eq!(resp.text().await.unwrap(), "http1 tls");

        handle.abort();
    }
}

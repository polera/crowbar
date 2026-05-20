pub mod ca;
pub mod cert_cache;
pub mod cert_gen;

use std::sync::Arc;

pub fn build_tls_client_config() -> Arc<rustls::ClientConfig> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    )
}

pub fn build_tls_h2_client_config() -> Arc<rustls::ClientConfig> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}

pub fn server_name_or_localhost(host: &str) -> rustls::pki_types::ServerName<'static> {
    rustls::pki_types::ServerName::try_from(host.to_owned())
        .unwrap_or_else(|_| rustls::pki_types::ServerName::try_from("localhost".to_owned()).unwrap())
}

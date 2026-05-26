pub mod ca;
pub mod cert_cache;
pub mod cert_gen;

use std::sync::Arc;

fn base_client_config() -> rustls::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

pub fn build_tls_client_config() -> Arc<rustls::ClientConfig> {
    Arc::new(base_client_config())
}

pub fn build_tls_h2_client_config() -> Arc<rustls::ClientConfig> {
    let mut config = base_client_config();
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}

pub fn server_name_or_localhost(host: &str) -> rustls::pki_types::ServerName<'static> {
    rustls::pki_types::ServerName::try_from(host.to_owned())
        .unwrap_or_else(|_| rustls::pki_types::ServerName::try_from("localhost".to_owned()).unwrap())
}

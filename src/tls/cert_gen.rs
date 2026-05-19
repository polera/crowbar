use std::sync::Arc;

use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;

use super::ca::CertificateAuthority;

pub fn generate_leaf_cert(
    hostname: &str,
    ca: &CertificateAuthority,
) -> anyhow::Result<CertifiedKey> {
    let leaf_key = KeyPair::generate()?;

    let mut params = CertificateParams::new(vec![hostname.to_string()])?;

    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, hostname);
    params.distinguished_name = dn;

    let leaf_cert = params.signed_by(&leaf_key, &ca.ca_cert, &ca.ca_key)?;

    let cert_der = CertificateDer::from(leaf_cert.der().to_vec());
    let ca_der = CertificateDer::from(ca.ca_cert.der().to_vec());
    let cert_chain = vec![cert_der, ca_der];

    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)?;

    Ok(CertifiedKey::new(cert_chain, signing_key))
}

pub fn build_server_config(certified_key: Arc<CertifiedKey>) -> anyhow::Result<Arc<rustls::ServerConfig>> {
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SingleCertResolver(certified_key)));

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(Arc::new(config))
}

#[derive(Debug)]
struct SingleCertResolver(Arc<CertifiedKey>);

impl rustls::server::ResolvesServerCert for SingleCertResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

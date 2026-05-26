use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, IsCa, KeyPair,
    KeyUsagePurpose,
};
use tracing::info;

pub struct CertificateAuthority {
    pub ca_cert: Certificate,
    pub ca_key: KeyPair,
    pub ca_cert_pem: String,
}

impl CertificateAuthority {
    pub fn load_or_generate() -> anyhow::Result<Self> {
        let dir = Self::config_dir()?;
        std::fs::create_dir_all(&dir)?;

        let cert_path = dir.join("ca.pem");
        let key_path = dir.join("ca.key");

        if cert_path.exists() && key_path.exists() {
            info!("Loading CA from {}", dir.display());
            return Self::load_from_disk(&cert_path, &key_path);
        }

        info!("Generating new CA certificate in {}", dir.display());
        let ca = Self::generate()?;
        ca.save_to_disk(&cert_path, &key_path)?;

        info!(
            "CA certificate saved. Install {} in your browser/OS trust store.",
            cert_path.display()
        );

        Ok(ca)
    }

    fn ca_params() -> CertificateParams {
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "Crowbar CA");
        dn.push(rcgen::DnType::OrganizationName, "Crowbar Proxy");

        let mut params = CertificateParams::default();
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params
    }

    fn generate() -> anyhow::Result<Self> {
        let ca_key = KeyPair::generate()?;
        let ca_cert = Self::ca_params().self_signed(&ca_key)?;
        let ca_cert_pem = ca_cert.pem();

        Ok(Self {
            ca_cert,
            ca_key,
            ca_cert_pem,
        })
    }

    fn load_from_disk(cert_path: &Path, key_path: &Path) -> anyhow::Result<Self> {
        let key_pem = std::fs::read_to_string(key_path)?;
        let ca_key = KeyPair::from_pem(&key_pem)?;
        let ca_cert = Self::ca_params().self_signed(&ca_key)?;
        let ca_cert_pem = std::fs::read_to_string(cert_path)?;

        Ok(Self {
            ca_cert,
            ca_key,
            ca_cert_pem,
        })
    }

    fn save_to_disk(&self, cert_path: &Path, key_path: &Path) -> anyhow::Result<()> {
        std::fs::write(cert_path, &self.ca_cert_pem)?;
        std::fs::write(key_path, self.ca_key.serialize_pem())?;
        Ok(())
    }

    fn config_dir() -> anyhow::Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
        Ok(home.join(".crowbar"))
    }
}

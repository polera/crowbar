use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, IsCa, KeyPair,
    KeyUsagePurpose,
};
use tracing::info;

pub struct CertificateAuthority {
    /// Re-derived signing cert used only as the issuer for leaf cert generation.
    /// Serial/validity may differ from `ca_cert_pem` since rcgen cannot parse existing PEM certs.
    pub ca_cert: Certificate,
    pub ca_key: KeyPair,
    /// On-disk PEM — the canonical cert for trust store installation.
    pub ca_cert_pem: String,
}

impl CertificateAuthority {
    pub fn load_or_generate() -> anyhow::Result<Self> {
        let dir = Self::config_dir()?;
        Self::load_or_generate_in(&dir)
    }

    fn load_or_generate_in(dir: &Path) -> anyhow::Result<Self> {
        if let Ok(metadata) = std::fs::symlink_metadata(dir) {
            anyhow::ensure!(
                !metadata.file_type().is_symlink(),
                "Refusing symlinked CA directory"
            );
        }
        crate::fs_security::ensure_private_dir(dir)?;

        // Keep the lock file's inode stable: atomically replacing it would let
        // different processes lock different files. Dropping the file unlocks it.
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(dir.join("ca.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        crate::fs_security::harden_private_tree(dir)?;

        let cert_path = dir.join("ca.pem");
        let key_path = dir.join("ca.key");

        match (cert_path.exists(), key_path.exists()) {
            (true, true) => {
                info!("Loading CA from {}", dir.display());
                return Self::load_from_disk(&cert_path, &key_path);
            }
            (false, false) => {}
            _ => anyhow::bail!(
                "Incomplete CA in {}; refusing to overwrite existing CA material",
                dir.display()
            ),
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

    pub(crate) fn generate() -> anyhow::Result<Self> {
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
        use rustls::pki_types::{
            CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, pem::PemObject,
        };
        let certified_key = rustls::sign::CertifiedKey::from_der(
            vec![CertificateDer::from_pem_slice(ca_cert_pem.as_bytes())?],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ca_key.serialize_der())),
            &rustls::crypto::ring::default_provider(),
        )
        .map_err(|error| anyhow::anyhow!("Invalid CA certificate/key pair: {error}"))?;
        certified_key
            .keys_match()
            .map_err(|error| anyhow::anyhow!("Invalid CA certificate/key pair: {error}"))?;

        Ok(Self {
            ca_cert,
            ca_key,
            ca_cert_pem,
        })
    }

    fn save_to_disk(&self, cert_path: &Path, key_path: &Path) -> anyhow::Result<()> {
        crate::fs_security::write_private(cert_path, &self.ca_cert_pem)?;
        crate::fs_security::write_private(key_path, self.ca_key.serialize_pem())?;
        Ok(())
    }

    fn config_dir() -> anyhow::Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
        Ok(home.join(".crowbar"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_ca_is_reused_and_mismatched_keys_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let first = CertificateAuthority::load_or_generate_in(directory.path()).unwrap();
        let loaded = CertificateAuthority::load_or_generate_in(directory.path()).unwrap();
        assert_eq!(first.ca_cert_pem, loaded.ca_cert_pem);
        assert_eq!(first.ca_key.serialize_der(), loaded.ca_key.serialize_der());

        let other = CertificateAuthority::generate().unwrap();
        crate::fs_security::write_private(
            &directory.path().join("ca.key"),
            other.ca_key.serialize_pem(),
        )
        .unwrap();
        let error = CertificateAuthority::load_or_generate_in(directory.path())
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("Invalid CA certificate/key pair")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("ca.pem")).unwrap(),
            first.ca_cert_pem
        );
    }

    #[test]
    fn incomplete_ca_is_not_silently_replaced() {
        for existing in ["ca.pem", "ca.key"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(existing);
            crate::fs_security::write_private(&path, "existing material").unwrap();
            let error = CertificateAuthority::load_or_generate_in(directory.path())
                .err()
                .unwrap();
            assert!(error.to_string().contains("Incomplete CA"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "existing material");
        }
    }

    // Invoked only by concurrent_processes_share_one_ca, in independent processes.
    #[test]
    #[ignore = "subprocess entry point exercised by concurrent_processes_share_one_ca"]
    fn ca_initialization_worker() {
        let Some(root) = std::env::var_os("CROWBAR_CA_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let output = PathBuf::from(std::env::var_os("CROWBAR_CA_TEST_OUTPUT").unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.join("start").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "start barrier timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let ca = CertificateAuthority::load_or_generate_in(&root.join("ca")).unwrap();
        std::fs::write(output, ca.ca_cert_pem).unwrap();
    }

    #[test]
    fn concurrent_processes_share_one_ca() {
        let directory = tempfile::tempdir().unwrap();
        let mut children = Vec::new();
        for index in 0..6 {
            let output = directory.path().join(format!("observed-{index}.pem"));
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tls::ca::tests::ca_initialization_worker",
                    "--ignored",
                ])
                .env("CROWBAR_CA_TEST_ROOT", directory.path())
                .env("CROWBAR_CA_TEST_OUTPUT", &output)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            children.push((child, output));
        }
        std::fs::write(directory.path().join("start"), "").unwrap();
        for (child, _) in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let ca = CertificateAuthority::load_or_generate_in(&directory.path().join("ca")).unwrap();
        for (_, output) in children {
            assert_eq!(std::fs::read_to_string(output).unwrap(), ca.ca_cert_pem);
        }
    }
}

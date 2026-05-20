use std::sync::Arc;

use lru::LruCache;
use rustls::sign::CertifiedKey;
use std::num::NonZeroUsize;
use tokio::sync::Mutex;

use super::ca::CertificateAuthority;
use super::cert_gen;

pub struct CertCache {
    cache: Mutex<LruCache<String, Arc<CertifiedKey>>>,
    ca: Arc<CertificateAuthority>,
}

impl CertCache {
    pub fn new(ca: Arc<CertificateAuthority>) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(1000).unwrap())),
            ca,
        }
    }

    pub async fn get_or_generate(&self, hostname: &str) -> anyhow::Result<Arc<CertifiedKey>> {
        let mut cache = self.cache.lock().await;

        if let Some(key) = cache.get(hostname) {
            return Ok(Arc::clone(key));
        }

        let certified_key = Arc::new(cert_gen::generate_leaf_cert(hostname, &self.ca)?);
        cache.put(hostname.to_string(), Arc::clone(&certified_key));

        Ok(certified_key)
    }
}

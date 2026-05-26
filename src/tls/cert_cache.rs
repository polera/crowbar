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
        // Phase 1: Check cache under lock
        {
            let mut cache = self.cache.lock().await;
            if let Some(key) = cache.get(hostname) {
                return Ok(Arc::clone(key));
            }
        }
        // Lock released here — other hostnames can proceed concurrently

        // Phase 2: Generate cert without holding the lock (CPU-intensive)
        let certified_key = Arc::new(cert_gen::generate_leaf_cert(hostname, &self.ca)?);

        // Phase 3: Re-acquire lock and insert.
        // A concurrent request for the same hostname may have already inserted;
        // overwriting with an equivalent cert is harmless and simpler than
        // coordinating with a per-host lock or futures map.
        {
            let mut cache = self.cache.lock().await;
            cache.put(hostname.to_string(), Arc::clone(&certified_key));
        }

        Ok(certified_key)
    }
}

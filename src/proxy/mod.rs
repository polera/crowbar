pub mod handler;
pub mod intercept;
pub mod repeater;
pub mod scope;
pub mod server;
pub mod tunnel;
pub mod ws_relay;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::channel::ProxyToUi;
use crate::proxy::intercept::InterceptState;
use crate::proxy::scope::Scope;
use crate::rules::SharedRules;
use crate::tls::cert_cache::CertCache;

#[derive(Clone)]
pub struct ProxyContext {
    pub ui_tx: mpsc::UnboundedSender<ProxyToUi>,
    pub cert_cache: Arc<CertCache>,
    pub intercept: Arc<InterceptState>,
    pub scope: Arc<Scope>,
    pub rules: SharedRules,
}

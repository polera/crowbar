use thiserror::Error;

#[derive(Error, Debug)]
pub enum CrowbarError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] http::Error),

    #[error("Hyper error: {0}")]
    Hyper(#[from] hyper::Error),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("Certificate generation error: {0}")]
    CertGen(#[from] rcgen::Error),

    #[error("Proxy error: {0}")]
    Proxy(String),

    #[error("Channel closed")]
    ChannelClosed,
}

pub type Result<T> = std::result::Result<T, CrowbarError>;

//! TLS setup shared by the IMAP (`client.rs`) and SMTP (`smtp.rs`) hand-rolled clients.

use anyhow::Result;
use std::sync::Once;
use tokio_rustls::rustls::RootCertStore;

static INIT_CRYPTO_PROVIDER: Once = Once::new();

/// Rustls 0.23 requires a process-wide default `CryptoProvider`. Both `ring` and
/// `aws-lc-rs` end up enabled in this workspace's dependency tree (pulled in by
/// different crates), which makes rustls' own auto-detection ambiguous and panic.
/// Install one explicitly, once; if something else (e.g. sqlx) already installed a
/// default first, this is a no-op.
pub(super) fn ensure_crypto_provider() {
    INIT_CRYPTO_PROVIDER.call_once(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

pub(super) fn build_root_store() -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    let (added, _skipped) = store.add_parsable_certificates(native.certs);

    if added == 0 {
        anyhow::bail!("no usable native root certificates found");
    }

    Ok(store)
}

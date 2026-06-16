use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use rustls::ServerConfig;
use rustls_pemfile::{certs, pkcs8_private_keys};
use tokio_rustls::TlsAcceptor;
use tracing::info;

use crate::config::TlsConfig;

/// 핫 리로드 가능한 TLS Acceptor
pub struct HotTlsAcceptor {
    inner: ArcSwap<TlsAcceptor>,
    cfg:   TlsConfig,
}

impl HotTlsAcceptor {
    pub fn new(cfg: &TlsConfig) -> Result<Arc<Self>> {
        let acceptor = build_acceptor(cfg)?;
        Ok(Arc::new(Self {
            inner: ArcSwap::from_pointee(acceptor),
            cfg:   cfg.clone(),
        }))
    }

    /// 현재 acceptor 반환
    pub fn get(&self) -> Arc<TlsAcceptor> {
        self.inner.load_full()
    }

    /// 인증서 다시 로드 (무중단 핫 로테이션)
    pub fn reload(&self) -> Result<()> {
        let acceptor = build_acceptor(&self.cfg)?;
        self.inner.store(Arc::new(acceptor));
        info!("TLS certificates reloaded");
        Ok(())
    }
}

fn build_acceptor(cfg: &TlsConfig) -> Result<TlsAcceptor> {
    let cert_file = File::open(&cfg.cert)
        .with_context(|| format!("Cannot open cert file: {}", cfg.cert))?;
    let key_file  = File::open(&cfg.key)
        .with_context(|| format!("Cannot open key file: {}", cfg.key))?;

    let certs: Vec<rustls::pki_types::CertificateDer> =
        certs(&mut BufReader::new(cert_file))
            .collect::<Result<_, _>>()
            .context("Failed to parse TLS certificate")?;

    let keys: Vec<rustls::pki_types::PrivateKeyDer> =
        pkcs8_private_keys(&mut BufReader::new(key_file))
            .map(|r| r.map(rustls::pki_types::PrivateKeyDer::Pkcs8))
            .collect::<Result<_, _>>()
            .context("Failed to parse private key")?;

    let key = keys.into_iter().next()
        .context("No private key found in key file")?;

    let tls_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("Failed to build TLS config")?;

    Ok(TlsAcceptor::from(Arc::new(tls_config)))
}

/// 자체 서명 인증서 생성 (개발용)
pub fn generate_self_signed_cert(cert_path: &str, key_path: &str) -> Result<()> {
    use std::process::Command;

    // openssl이 있으면 사용, 없으면 스킵
    let status = Command::new("openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:4096",
            "-keyout", key_path,
            "-out", cert_path,
            "-days", "365",
            "-nodes",
            "-subj", "/CN=localhost",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            info!("Generated self-signed certificate: {}", cert_path);
            Ok(())
        }
        _ => {
            // openssl 없으면 더미 파일 생성 (HTTP only 모드)
            anyhow::bail!("openssl not available — run in HTTP-only mode")
        }
    }
}

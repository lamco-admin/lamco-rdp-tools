//! TLS certificate inspection for the rdpsee `cert` verb.
//!
//! Negotiates an enhanced-security (TLS) protocol via the X.224 layer, performs
//! the TLS handshake through `ironrdp-tls`, and reports the server certificate.
//! `ironrdp_tls::upgrade` already returns a parsed `x509_cert::Certificate`, so
//! the certificate fields come straight off that with no extra parsing crate.
//!
//! The negotiated TLS version and cipher suite are intentionally absent: the
//! published `ironrdp-tls` API does not surface them. Tracked upstream by
//! Devolutions/IronRDP PR #1384 (adds a backend-neutral `negotiated()`
//! accessor); they will be added here once it lands in a release.

use std::{fmt::Write as _, time::Duration};

use anyhow::{Result, bail};
use ironrdp_pdu::nego::{ConnectionConfirm, ResponseFlags, SecurityProtocol};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use x509_cert::{
    Certificate,
    der::Encode as _,
    ext::pkix::{SubjectAltName, name::GeneralName},
};

use crate::{connection::Destination, probe};

/// A server's TLS certificate, as reported by `rdpsee cert`.
#[derive(Debug, Serialize)]
pub(crate) struct CertReport {
    pub server: String,
    pub subject: String,
    pub issuer: String,
    pub self_signed: bool,
    pub not_before: String,
    pub not_after: String,
    pub serial: String,
    pub signature_algorithm: String,
    pub public_key: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subject_alt_names: Vec<String>,
    pub sha256_fingerprint: String,
}

impl CertReport {
    pub(crate) fn print_human(&self) {
        println!("server:       {}", self.server);
        println!("subject:      {}", self.subject);
        println!("issuer:       {}", self.issuer);
        println!(
            "self-signed:  {}",
            if self.self_signed { "yes" } else { "no" }
        );
        println!("not before:   {}", self.not_before);
        println!("not after:    {}", self.not_after);
        println!("serial:       {}", self.serial);
        println!("signature:    {}", self.signature_algorithm);
        println!("public key:   {}", self.public_key);
        if !self.subject_alt_names.is_empty() {
            println!("subject alt:  {}", self.subject_alt_names.join(", "));
        }
        println!("sha256:       {}", self.sha256_fingerprint);
    }
}

/// A completed RDP-over-TLS handshake without authentication: the negotiated
/// security parameters plus the server certificate.
pub(crate) struct TlsHandshake {
    pub protocol: SecurityProtocol,
    pub flags: ResponseFlags,
    pub certificate: Certificate,
}

/// Negotiate an enhanced-security protocol and complete the TLS handshake,
/// returning the negotiated parameters and the server certificate. No
/// `CredSSP`, no session. Shared by the `cert` and `id` verbs.
pub(crate) async fn connect_tls(dest: &Destination, timeout: Duration) -> Result<TlsHandshake> {
    let addr = dest.addr_string();

    let mut stream = tokio::time::timeout(timeout, TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow::anyhow!("connect timeout"))?
        .map_err(|e| anyhow::anyhow!("TCP connect to {addr}: {e}"))?;

    let requested = SecurityProtocol::SSL | SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX;
    let (protocol, flags) = match probe::negotiate_protocol(&mut stream, requested, timeout).await?
    {
        ConnectionConfirm::Response { protocol, flags } if !protocol.is_standard_rdp_security() => {
            (protocol, flags)
        }
        ConnectionConfirm::Response { .. } => {
            bail!("server selected standard RDP security; no TLS to inspect");
        }
        ConnectionConfirm::Failure { code } => {
            bail!(
                "server rejected negotiation (failure code {})",
                u32::from(code)
            );
        }
    };

    // The TLS handshake follows the negotiation on the same stream.
    let (_tls_stream, certificate) = ironrdp_tls::upgrade(stream, &dest.name)
        .await
        .map_err(|e| anyhow::anyhow!("TLS upgrade: {e}"))?;

    Ok(TlsHandshake {
        protocol,
        flags,
        certificate,
    })
}

/// Negotiate TLS and report the server certificate (the `cert` verb).
pub(crate) async fn fetch_cert(dest: &Destination, timeout: Duration) -> Result<CertReport> {
    let handshake = connect_tls(dest, timeout).await?;
    build_report(dest.addr_string(), &handshake.certificate)
}

fn build_report(server: String, cert: &Certificate) -> Result<CertReport> {
    let tbs = &cert.tbs_certificate;
    let subject = tbs.subject.to_string();
    let issuer = tbs.issuer.to_string();
    let der = cert
        .to_der()
        .map_err(|e| anyhow::anyhow!("re-encode certificate DER: {e}"))?;

    Ok(CertReport {
        self_signed: subject == issuer,
        server,
        subject,
        issuer,
        not_before: tbs.validity.not_before.to_string(),
        not_after: tbs.validity.not_after.to_string(),
        serial: tbs.serial_number.to_string(),
        signature_algorithm: oid_name(&tbs.signature.oid.to_string()),
        public_key: oid_name(&tbs.subject_public_key_info.algorithm.oid.to_string()),
        subject_alt_names: subject_alt_names(tbs),
        sha256_fingerprint: hex_lower(&Sha256::digest(&der)),
    })
}

fn subject_alt_names(tbs: &x509_cert::TbsCertificate) -> Vec<String> {
    match tbs.get::<SubjectAltName>() {
        Ok(Some((_critical, san))) => san.0.iter().map(general_name).collect(),
        _ => Vec::new(),
    }
}

fn general_name(name: &GeneralName) -> String {
    match name {
        GeneralName::DnsName(dns) => format!("DNS:{dns}"),
        GeneralName::Rfc822Name(email) => format!("email:{email}"),
        GeneralName::UniformResourceIdentifier(uri) => format!("URI:{uri}"),
        GeneralName::IpAddress(ip) => format!("IP:{}", format_ip(ip.as_bytes())),
        other => format!("{other:?}"),
    }
}

/// Map the most common algorithm OIDs to friendly names, falling back to the
/// dotted OID for anything else.
pub(crate) fn oid_name(oid: &str) -> String {
    let name = match oid {
        "1.2.840.113549.1.1.1" => "RSA",
        "1.2.840.113549.1.1.11" => "SHA256-RSA",
        "1.2.840.113549.1.1.12" => "SHA384-RSA",
        "1.2.840.113549.1.1.13" => "SHA512-RSA",
        "1.2.840.113549.1.1.5" => "SHA1-RSA",
        "1.2.840.10045.2.1" => "ECDSA",
        "1.2.840.10045.4.3.2" => "ECDSA-SHA256",
        "1.2.840.10045.4.3.3" => "ECDSA-SHA384",
        "1.3.101.112" => "Ed25519",
        _ => return oid.to_owned(),
    };
    name.to_owned()
}

fn format_ip(bytes: &[u8]) -> String {
    match bytes.len() {
        4 => std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string(),
        16 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(bytes);
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => hex_lower(bytes),
    }
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

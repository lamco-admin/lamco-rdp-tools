//! Server fingerprinting for the rdpsee `id` verb.
//!
//! Combines the pre-auth RDP security negotiation (selected protocol + response
//! flags) with the TLS certificate's algorithm profile into a stable,
//! JA4-style server fingerprint: a short categorical prefix plus a truncated
//! hash of the full observable feature set. It identifies a server's
//! configuration/type, not a single connection (the exact-instance identity is
//! the certificate's SHA-256, reported separately).

use std::time::Duration;

use anyhow::Result;
use ironrdp_pdu::nego::{ResponseFlags, SecurityProtocol};
use serde::Serialize;
use sha2::{Digest, Sha256};
use x509_cert::der::Encode as _;

use crate::{
    cert::{self, hex_lower, oid_name},
    connection::Destination,
};

/// A server fingerprint, as reported by `rdpsee id`.
#[derive(Debug, Serialize)]
pub(crate) struct FingerprintReport {
    pub server: String,
    /// JA4-style fingerprint: categorical prefix + truncated feature hash.
    pub fingerprint: String,
    pub security: String,
    pub nla_required: bool,
    pub egfx_capable: bool,
    pub public_key: String,
    pub signature_algorithm: String,
    pub self_signed: bool,
    /// SHA-256 of the exact server certificate (instance identity).
    pub cert_sha256: String,
}

impl FingerprintReport {
    pub(crate) fn print_human(&self) {
        println!("server:       {}", self.server);
        println!("fingerprint:  {}", self.fingerprint);
        println!("security:     {}", self.security);
        println!(
            "nla:          {}",
            if self.nla_required {
                "required"
            } else {
                "not required"
            }
        );
        println!(
            "egfx:         {}",
            if self.egfx_capable { "yes" } else { "no" }
        );
        println!("public key:   {}", self.public_key);
        println!("signature:    {}", self.signature_algorithm);
        println!(
            "self-signed:  {}",
            if self.self_signed { "yes" } else { "no" }
        );
        println!("cert sha256:  {}", self.cert_sha256);
    }
}

/// Negotiate, complete the TLS handshake, and compute a server fingerprint.
pub(crate) async fn fingerprint(
    dest: &Destination,
    timeout: Duration,
) -> Result<FingerprintReport> {
    let handshake = cert::connect_tls(dest, timeout).await?;
    let tbs = &handshake.certificate.tbs_certificate;

    let public_key_oid = tbs.subject_public_key_info.algorithm.oid.to_string();
    let signature_oid = tbs.signature.oid.to_string();
    let self_signed = tbs.subject == tbs.issuer;

    let der = handshake
        .certificate
        .to_der()
        .map_err(|e| anyhow::anyhow!("re-encode certificate DER: {e}"))?;
    let cert_sha256 = hex_lower(&Sha256::digest(&der));

    // Canonical feature string -> stable hash. Use raw bit/OID values so the
    // hash is independent of any name formatting.
    let canonical = format!(
        "sec={};flags={};pk={};sig={};self={}",
        handshake.protocol.bits(),
        handshake.flags.bits(),
        public_key_oid,
        signature_oid,
        self_signed,
    );
    let feature_hash = hex_lower(&Sha256::digest(canonical.as_bytes()));

    let nla_required = handshake.protocol.contains(SecurityProtocol::HYBRID)
        || handshake.protocol.contains(SecurityProtocol::HYBRID_EX);
    let egfx_capable = handshake
        .flags
        .contains(ResponseFlags::DYNVC_GFX_PROTOCOL_SUPPORTED);

    let fingerprint = format!(
        "rdp_{}{}_{}{}/{}",
        security_tag(handshake.protocol),
        capability_tags(handshake.flags),
        if self_signed { 'S' } else { 'C' },
        key_tag(&public_key_oid),
        &feature_hash[..16],
    );

    Ok(FingerprintReport {
        server: dest.addr_string(),
        fingerprint,
        security: handshake.protocol.to_string(),
        nla_required,
        egfx_capable,
        public_key: oid_name(&public_key_oid),
        signature_algorithm: oid_name(&signature_oid),
        self_signed,
        cert_sha256,
    })
}

fn security_tag(protocol: SecurityProtocol) -> char {
    if protocol.contains(SecurityProtocol::HYBRID_EX) {
        'q'
    } else if protocol.contains(SecurityProtocol::HYBRID) {
        'h'
    } else if protocol.contains(SecurityProtocol::SSL) {
        's'
    } else {
        'r'
    }
}

fn capability_tags(flags: ResponseFlags) -> String {
    let mut out = String::new();
    if flags.contains(ResponseFlags::DYNVC_GFX_PROTOCOL_SUPPORTED) {
        out.push('g');
    }
    if flags.contains(ResponseFlags::RESTRICTED_ADMIN_MODE_SUPPORTED) {
        out.push('a');
    }
    if flags.contains(ResponseFlags::REDIRECTED_AUTHENTICATION_MODE_SUPPORTED) {
        out.push('d');
    }
    if flags.contains(ResponseFlags::EXTENDED_CLIENT_DATA_SUPPORTED) {
        out.push('e');
    }
    if out.is_empty() {
        out.push('-');
    }
    out
}

fn key_tag(oid: &str) -> char {
    match oid {
        "1.2.840.113549.1.1.1" => 'r', // RSA
        "1.2.840.10045.2.1" => 'e',    // ECDSA
        "1.3.101.112" => 'd',          // Ed25519
        _ => 'x',
    }
}

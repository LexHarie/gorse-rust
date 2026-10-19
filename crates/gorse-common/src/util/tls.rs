// Ports: common/util/tls.go @ add21b5

//! Mutual TLS between Gorse nodes. Go builds gRPC credentials with
//! `advancedtls` in `CertVerification` mode; this module builds the
//! equivalent `rustls` configurations:
//!
//! - both sides present the certificate and key from [`TlsConfig`];
//! - each side checks the peer's chain against the CA file only, as Go's
//!   `x509.Verify` does: the server requires a client certificate valid for
//!   client authentication, the client requires a server certificate valid
//!   for server authentication, and **no host name is checked**, as
//!   `CertVerification` skips it;
//! - both offer the `h2` ALPN protocol, as gRPC's credentials do.
//!
//! Go also checks the CA certificate at the end of the chain: it must be valid
//! now, its extended key usage, if any, must allow the purpose, and the chain
//! must respect its path length limit. A `webpki` trust anchor keeps only the
//! name and key, so these checks run on each chain `webpki` finds, and a
//! chain that fails them makes it try the next, as Go does.
//!
//! Go clients send the dial target, `host:port`, as the TLS server name
//! (advancedtls passes it on unchanged). Stock `rustls` refuses such a name,
//! so the workspace builds against the patched copy in `third_party/rustls`,
//! which ignores it instead (see its `GORSE.md`).
//!
//! The cryptography is `aws-lc-rs`'s, which, like Go, handles RSA, ECDSA
//! P-256, P-384 and P-521, and Ed25519 keys; the certificates Gorse's own
//! tests generate are P-521.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, aws_lc_rs};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, TrustAnchor, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, OtherError,
    RootCertStore, ServerConfig, SignatureScheme,
};
use webpki::{
    EndEntityCert, ExtendedKeyUsageValidator, KeyPurposeIdIter, KeyUsage,
    RequiredEkuNotFoundContext, VerifiedPath,
};
use x509_parser::prelude::{FromDer, X509Certificate, X509Version};

/// Go's `util.TLSConfig`: paths of the PEM files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsConfig {
    /// The certificate authority that signs both sides' certificates.
    pub ssl_ca: String,
    /// This node's certificate chain.
    pub ssl_cert: String,
    /// This node's private key.
    pub ssl_key: String,
}

/// Why a TLS configuration could not be built.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    /// A file could not be read.
    #[error("open {path}: {source}")]
    Read {
        /// The file.
        path: String,
        /// The I/O error.
        source: std::io::Error,
    },
    /// The CA file holds no usable certificate (Go's `AppendCertsFromPEM`
    /// failing).
    #[error("failed to append certificate")]
    AppendCertificate,
    /// The certificate file holds no certificate.
    #[error("tls: failed to find any PEM data in certificate input")]
    NoCertificate,
    /// The key file holds no private key.
    #[error("tls: failed to find any PEM data in key input")]
    NoKey,
    /// `rustls` rejected the certificate or key, for example a key that does
    /// not match the certificate.
    #[error("tls: {0}")]
    Rustls(#[from] rustls::Error),
}

fn read(path: &str) -> Result<Vec<u8>, TlsError> {
    fs::read(path).map_err(|source| TlsError::Read {
        path: path.to_owned(),
        source,
    })
}

/// What a certificate is checked for: the `x509.ExtKeyUsage` passed to
/// `x509.Verify`.
#[derive(Debug, Clone, Copy)]
enum Purpose {
    ServerAuth,
    ClientAuth,
}

const ANY_EKU: &[usize] = &[2, 5, 29, 37, 0];
const SERVER_AUTH: &[usize] = &[1, 3, 6, 1, 5, 5, 7, 3, 1];
const CLIENT_AUTH: &[usize] = &[1, 3, 6, 1, 5, 5, 7, 3, 2];

impl Purpose {
    fn usage(self) -> KeyUsage {
        match self {
            Purpose::ServerAuth => KeyUsage::server_auth(),
            Purpose::ClientAuth => KeyUsage::client_auth(),
        }
    }

    fn arcs(self) -> &'static [usize] {
        match self {
            Purpose::ServerAuth => SERVER_AUTH,
            Purpose::ClientAuth => CLIENT_AUTH,
        }
    }

    /// Go's rule for one certificate's extended key usage
    /// (`checkChainForKeyUsage` with one usage): no extension, the purpose,
    /// or `anyExtendedKeyUsage`. `webpki`'s own rule ignores the last.
    fn allows(self, eku: Option<&[Vec<usize>]>) -> Result<(), webpki::Error> {
        match eku {
            Some(eku)
                if !eku
                    .iter()
                    .any(|arcs| arcs == ANY_EKU || arcs == self.arcs()) =>
            {
                Err(webpki::Error::RequiredEkuNotFoundContext(
                    RequiredEkuNotFoundContext {
                        required: self.usage(),
                        present: eku.to_vec(),
                    },
                ))
            }
            _ => Ok(()),
        }
    }
}

/// [`Purpose::allows`] for `webpki`, which applies it to every certificate of
/// a chain below the root.
impl ExtendedKeyUsageValidator for Purpose {
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), webpki::Error> {
        let mut present = Vec::new();
        for id in iter {
            present.push(id?.to_decoded_oid());
        }
        self.allows((!present.is_empty()).then_some(&present[..]))
    }
}

/// The constraints Go's `CheckSignatureFrom` puts on an issuing certificate:
/// a version 3 certificate must have basic constraints with `cA` set, and a
/// key usage with any bit set must include `keyCertSign`.
fn may_sign(cert: &X509Certificate<'_>) -> Result<(), webpki::Error> {
    let constraints = cert
        .basic_constraints()
        .map_err(|_| webpki::Error::BadDer)?;
    let usage = cert.key_usage().map_err(|_| webpki::Error::BadDer)?;
    let ca = match constraints {
        Some(bc) => bc.value.ca,
        None => cert.version() != X509Version::V3,
    };
    let usage_ok = usage.is_none_or(|ku| ku.value.flags == 0 || ku.value.key_cert_sign());
    if ca && usage_ok {
        Ok(())
    } else {
        Err(webpki::Error::EndEntityUsedAsCa)
    }
}

/// A certificate from the CA file, with the facts Go's `x509.Verify` checks
/// on a root and a `webpki` trust anchor does not keep.
#[derive(Debug)]
struct Root {
    anchor: TrustAnchor<'static>,
    /// Validity period, Unix seconds.
    not_before: i64,
    not_after: i64,
    /// The extended key usage OIDs, as arcs, or `None` without the
    /// extension.
    eku: Option<Vec<Vec<usize>>>,
    /// The basic constraints' path length limit (`MaxPathLen`), if any.
    max_path_len: Option<u32>,
    /// [`may_sign`].
    may_sign: Result<(), webpki::Error>,
}

impl Root {
    /// `x509.ParseCertificate`, then the trust anchor `webpki` builds chains
    /// to.
    fn parse(der: &CertificateDer<'_>) -> Option<Root> {
        let (_, cert) = X509Certificate::from_der(der).ok()?;
        let eku = cert.extended_key_usage().ok()?.map(|ext| {
            let e = ext.value;
            let known = [
                (e.any, ANY_EKU),
                (e.server_auth, SERVER_AUTH),
                (e.client_auth, CLIENT_AUTH),
                (e.code_signing, &[1, 3, 6, 1, 5, 5, 7, 3, 3]),
                (e.email_protection, &[1, 3, 6, 1, 5, 5, 7, 3, 4]),
                (e.time_stamping, &[1, 3, 6, 1, 5, 5, 7, 3, 8]),
                (e.ocsp_signing, &[1, 3, 6, 1, 5, 5, 7, 3, 9]),
            ];
            let known = known
                .into_iter()
                .filter(|(has, _)| *has)
                .map(|(_, arcs)| arcs.to_vec());
            let other = e.other.iter().map(|oid| {
                oid.iter()
                    .map(|arcs| arcs.map(|a| a as usize).collect())
                    .unwrap_or_default()
            });
            known.chain(other).collect()
        });
        let max_path_len = cert
            .basic_constraints()
            .ok()?
            .and_then(|bc| bc.value.path_len_constraint);
        let anchor = webpki::anchor_from_trusted_cert(der).ok()?.to_owned();
        Some(Root {
            anchor,
            not_before: cert.validity().not_before.timestamp(),
            not_after: cert.validity().not_after.timestamp(),
            eku,
            max_path_len,
            may_sign: may_sign(&cert),
        })
    }

    /// Whether Go accepts this certificate as the root of a chain with
    /// `intermediates` CA certificates below it: it may sign certificates,
    /// `now` lies in its validity period (both ends included), its extended
    /// key usage allows `purpose`, and the chain is no longer than its path
    /// length limit.
    fn check(
        &self,
        now: UnixTime,
        purpose: Purpose,
        intermediates: usize,
    ) -> Result<(), webpki::Error> {
        self.may_sign.clone()?;
        let secs = i64::try_from(now.as_secs()).unwrap_or(i64::MAX);
        let unix =
            |t: i64| UnixTime::since_unix_epoch(Duration::from_secs(u64::try_from(t).unwrap_or(0)));
        if secs < self.not_before {
            return Err(webpki::Error::CertNotValidYet {
                time: now,
                not_before: unix(self.not_before),
            });
        }
        if secs > self.not_after {
            return Err(webpki::Error::CertExpired {
                time: now,
                not_after: unix(self.not_after),
            });
        }
        purpose.allows(self.eku.as_deref())?;
        if self
            .max_path_len
            .is_some_and(|max| intermediates > max as usize)
        {
            return Err(webpki::Error::PathLenConstraintViolated);
        }
        Ok(())
    }

    fn is(&self, anchor: &TrustAnchor<'_>) -> bool {
        self.anchor.subject == anchor.subject
            && self.anchor.subject_public_key_info == anchor.subject_public_key_info
    }
}

/// The certificates of the CA file (Go's `x509.CertPool`).
#[derive(Debug)]
struct Roots {
    roots: Vec<Root>,
    anchors: Vec<TrustAnchor<'static>>,
    subjects: Vec<DistinguishedName>,
}

impl Roots {
    /// Reads the CA file. Like Go's `AppendCertsFromPEM`, blocks that are not
    /// certificates, or do not parse, are skipped, and it fails only when no
    /// certificate was added.
    fn load(path: &str) -> Result<Roots, TlsError> {
        let pem = read(path)?;
        let roots: Vec<Root> = CertificateDer::pem_slice_iter(&pem)
            .filter_map(Result::ok)
            .filter_map(|der| Root::parse(&der))
            .collect();
        if roots.is_empty() {
            return Err(TlsError::AppendCertificate);
        }
        let anchors: Vec<_> = roots.iter().map(|r| r.anchor.clone()).collect();
        let subjects = RootCertStore {
            roots: anchors.clone(),
        }
        .subjects();
        Ok(Roots {
            roots,
            anchors,
            subjects,
        })
    }

    /// `x509.Verify` with `o.Roots` and `o.KeyUsages = []ExtKeyUsage{purpose}`:
    /// some chain from `end_entity` through `intermediates` must have
    /// intermediates that may sign ([`may_sign`]) and end at a CA certificate
    /// that Go accepts as a root ([`Root::check`]).
    fn verify(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
        purpose: Purpose,
        algorithms: &WebPkiSupportedAlgorithms,
    ) -> Result<(), rustls::Error> {
        let cert = EndEntityCert::try_from(end_entity).map_err(pki_error)?;
        // webpki tries the next chain when this rejects one.
        let go_ok = |path: &VerifiedPath<'_>| {
            let mut below = 0;
            for cert in path.intermediate_certificates() {
                let der = cert.der();
                let (_, cert) =
                    X509Certificate::from_der(&der).map_err(|_| webpki::Error::BadDer)?;
                may_sign(&cert)?;
                below += 1;
            }
            let mut err = webpki::Error::UnknownIssuer;
            for root in self.roots.iter().filter(|r| r.is(path.anchor())) {
                match root.check(now, purpose, below) {
                    Ok(()) => return Ok(()),
                    Err(e) => err = e,
                }
            }
            Err(err)
        };
        cert.verify_for_usage(
            algorithms.all,
            &self.anchors,
            intermediates,
            now,
            purpose,
            None,
            Some(&go_ok),
        )
        .map_err(pki_error)?;
        Ok(())
    }
}

/// A `webpki` error as the `rustls` error the handshake reports, as `rustls`'s
/// own verifiers map the common ones.
fn pki_error(e: webpki::Error) -> rustls::Error {
    use webpki::Error as E;
    let e = match e {
        E::BadDer | E::BadDerTime | E::TrailingData(_) => CertificateError::BadEncoding,
        E::CertExpired { time, not_after } => CertificateError::ExpiredContext { time, not_after },
        E::CertNotValidYet { time, not_before } => {
            CertificateError::NotValidYetContext { time, not_before }
        }
        E::UnknownIssuer => CertificateError::UnknownIssuer,
        E::InvalidSignatureForPublicKey => CertificateError::BadSignature,
        E::RequiredEkuNotFoundContext(_) => CertificateError::InvalidPurpose,
        other => CertificateError::Other(OtherError(Arc::new(other))),
    };
    rustls::Error::InvalidCertificate(e)
}

/// Go's `tls.LoadX509KeyPair`: the certificate chain and the first private
/// key (PKCS#1, PKCS#8 or SEC1).
fn load_key_pair(
    o: &TlsConfig,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), TlsError> {
    let cert_pem = read(&o.ssl_cert)?;
    let chain: Vec<_> = CertificateDer::pem_slice_iter(&cert_pem)
        .filter_map(Result::ok)
        .collect();
    if chain.is_empty() {
        return Err(TlsError::NoCertificate);
    }
    let key_pem = read(&o.ssl_key)?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|_| TlsError::NoKey)?;
    Ok((chain, key))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(aws_lc_rs::default_provider())
}

/// `NewServerCreds(o)`: a server configuration that presents `o`'s
/// certificate and requires a client certificate signed by `o`'s CA.
pub fn new_server_config(o: &TlsConfig) -> Result<ServerConfig, TlsError> {
    let roots = Roots::load(&o.ssl_ca)?;
    let (chain, key) = load_key_pair(o)?;
    let provider = provider();
    let verifier = ClientVerifier {
        roots,
        provider: provider.clone(),
    };
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(chain, key)?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

/// `NewClientCreds(o)`: a client configuration that presents `o`'s
/// certificate and accepts a server certificate chained to `o`'s CA, whatever
/// host name it names.
pub fn new_client_config(o: &TlsConfig) -> Result<ClientConfig, TlsError> {
    let roots = Roots::load(&o.ssl_ca)?;
    let (chain, key) = load_key_pair(o)?;
    let provider = provider();
    let verifier = ServerVerifier {
        roots,
        algorithms: provider.signature_verification_algorithms,
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(chain, key)?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

/// `advancedtls.CertVerification` on the client: the server's chain must
/// lead to an accepted root and be valid for server authentication now; the
/// host name is not compared.
#[derive(Debug)]
struct ServerVerifier {
    roots: Roots,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for ServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.roots.verify(
            end_entity,
            intermediates,
            now,
            Purpose::ServerAuth,
            &self.algorithms,
        )?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// `advancedtls.CertVerification` with `RequireClientCert` on the server: a
/// client certificate is required, and its chain must lead to an accepted
/// root and be valid for client authentication now. The certificate request
/// names every CA in the file, as Go's does.
#[derive(Debug)]
struct ClientVerifier {
    roots: Roots,
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for ClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.roots.subjects
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.roots.verify(
            end_entity,
            intermediates,
            now,
            Purpose::ClientAuth,
            &self.provider.signature_verification_algorithms,
        )?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

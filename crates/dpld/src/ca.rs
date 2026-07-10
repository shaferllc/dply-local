//! A local certificate authority for trusted HTTPS on `.test` sites.
//!
//! On first use we generate a CA key + self-signed CA cert under
//! `~/.dpl/certs`. The CA cert (`ca.pem`) is what the user trusts once (via the
//! privileged helper / `dpl trust`); thereafter the daemon mints a short leaf
//! certificate per `.test` host on demand, signed by this CA, so browsers
//! accept `https://<name>.test` with no warning.

use std::path::PathBuf;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub struct LocalCa {
    cert: rcgen::Certificate,
    key: KeyPair,
}

impl LocalCa {
    /// Load the CA from `~/.dpl/certs`, generating it on first run.
    pub fn load_or_create() -> Result<Self> {
        let dir = dpl_core::paths::certs_dir(None)?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let key_path = dir.join("ca.key");
        let pem_path = dir.join("ca.pem");

        if key_path.is_file() && pem_path.is_file() {
            let key = KeyPair::from_pem(&std::fs::read_to_string(&key_path)?)
                .context("loading CA key")?;
            // Rebuild the issuer certificate from the same key + subject; a
            // leaf signed by it validates against the trusted ca.pem because
            // the subject DN and public key match.
            let cert = ca_params()?.self_signed(&key).context("rebuilding CA cert")?;
            return Ok(LocalCa { cert, key });
        }

        let key = KeyPair::generate().context("generating CA key")?;
        let cert = ca_params()?.self_signed(&key).context("self-signing CA")?;
        write_private(&key_path, &key.serialize_pem())?;
        std::fs::write(&pem_path, cert.pem()).with_context(|| format!("writing {}", pem_path.display()))?;
        tracing::info!(ca = %pem_path.display(), "generated local CA");
        Ok(LocalCa { cert, key })
    }

    /// Mint a leaf certificate for `host`, signed by the CA.
    pub fn issue(&self, host: &str) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        let key = KeyPair::generate().context("generating leaf key")?;
        let mut params =
            CertificateParams::new(vec![host.to_string()]).context("leaf params")?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host);
        params.distinguished_name = dn;

        let leaf = params
            .signed_by(&key, &self.cert, &self.key)
            .context("signing leaf cert")?;
        let cert_der = leaf.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        Ok((cert_der, key_der))
    }
}

/// Parameters for the CA certificate (same every time, so it can be rebuilt
/// from the stored key).
fn ca_params() -> Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::<String>::new()).context("ca params")?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "dpl local CA");
    dn.push(DnType::OrganizationName, "dpl");
    params.distinguished_name = dn;
    Ok(params)
}

/// Write a key file mode 0600.
fn write_private(path: &PathBuf, pem: &str) -> Result<()> {
    std::fs::write(path, pem).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

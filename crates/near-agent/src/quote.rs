//! Just enough TDX quote parsing to fetch collateral: the PCK certificate chain
//! embedded in the quote, and from its leaf certificate the FMSPC, issuing CA and
//! PPID. The contract does the actual verification.

use crate::AgentError;
use base64::{engine::general_purpose::STANDARD, Engine};

/// What the PCCS endpoints are keyed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PckInfo {
    /// Uppercase hex, 6 bytes.
    pub fmspc: String,
    /// `platform` or `processor`, as the `pckcrl?ca=` parameter expects.
    pub ca: &'static str,
}

const HEADER_LEN: usize = 48;
const TDX_TEE_TYPE: u32 = 0x81;
const TD10_REPORT_LEN: usize = 584;
const QE_REPORT_LEN: usize = 384;
const CERT_DATA_PCK_CHAIN: u16 = 5;
const CERT_DATA_QE_REPORT: u16 = 6;

/// DER of OID 1.2.840.113741.1.13.1.4 (Intel SGX extension: FMSPC).
const FMSPC_OID: [u8; 12] = [
    0x06, 0x0a, 0x2a, 0x86, 0x48, 0x86, 0xf8, 0x4d, 0x01, 0x0d, 0x01, 0x04,
];
/// DER of OID 1.2.840.113741.1.13.1.1 (Intel SGX extension: PPID).
const PPID_OID: [u8; 12] = [
    0x06, 0x0a, 0x2a, 0x86, 0x48, 0x86, 0xf8, 0x4d, 0x01, 0x0d, 0x01, 0x01,
];

pub fn pck_info(quote: &[u8]) -> Result<PckInfo, AgentError> {
    let leaf = first_certificate(pck_chain(quote)?)?;
    Ok(PckInfo {
        fmspc: hex::encode_upper(fmspc(&leaf)?),
        ca: pck_ca(&leaf),
    })
}

/// The platform's PPID from the leaf PCK certificate, lowercase hex: the value the
/// contract checks against its approved PPIDs (dcap-qvl reads the same extension).
pub fn ppid(quote: &[u8]) -> Result<String, AgentError> {
    let leaf = first_certificate(pck_chain(quote)?)?;
    Ok(hex::encode(sgx_extension(&leaf, &PPID_OID, "PPID", 16)?))
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], AgentError> {
        if self.buf.len() < n {
            return Err(AgentError::Quote("quote is truncated".into()));
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, AgentError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, AgentError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn sized(&mut self) -> Result<&'a [u8], AgentError> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}

/// The PEM PCK certificate chain (certification data type 5) from a v4 or v5 TDX quote.
fn pck_chain(quote: &[u8]) -> Result<&[u8], AgentError> {
    let mut r = Reader { buf: quote };
    let header = r.take(HEADER_LEN)?;
    let version = u16::from_le_bytes([header[0], header[1]]);
    let tee_type = u32::from_le_bytes(header[4..8].try_into().unwrap());
    if tee_type != TDX_TEE_TYPE {
        return Err(AgentError::Quote(format!(
            "not a TDX quote (TEE type {tee_type:#x})"
        )));
    }
    match version {
        4 => {
            r.take(TD10_REPORT_LEN)?;
        }
        5 => {
            r.u16()?;
            r.sized()?;
        }
        v => return Err(AgentError::Quote(format!("unsupported quote version {v}"))),
    }

    let mut sig = Reader { buf: r.sized()? };
    // ECDSA signature and attestation public key.
    sig.take(64 + 64)?;
    let mut cert_type = sig.u16()?;
    let mut cert_data = sig.sized()?;

    if cert_type == CERT_DATA_QE_REPORT {
        let mut qe = Reader { buf: cert_data };
        // QE report and its signature, then QE authentication data.
        qe.take(QE_REPORT_LEN + 64)?;
        let auth_len = qe.u16()? as usize;
        qe.take(auth_len)?;
        cert_type = qe.u16()?;
        cert_data = qe.sized()?;
    }
    if cert_type != CERT_DATA_PCK_CHAIN {
        return Err(AgentError::Quote(format!(
            "unsupported certification data type {cert_type}"
        )));
    }
    Ok(cert_data)
}

fn first_certificate(pem: &[u8]) -> Result<Vec<u8>, AgentError> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let text = String::from_utf8_lossy(pem);
    let start = text
        .find(BEGIN)
        .ok_or_else(|| AgentError::Quote("no certificate in PCK chain".into()))?
        + BEGIN.len();
    let len = text[start..]
        .find(END)
        .ok_or_else(|| AgentError::Quote("unterminated certificate in PCK chain".into()))?;
    let body: String = text[start..start + len]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    STANDARD
        .decode(body)
        .map_err(|e| AgentError::Quote(format!("PCK certificate is not base64: {e}")))
}

/// The value of SGX extension `oid` in `cert`: an OCTET STRING of `len` bytes.
fn sgx_extension<'a>(
    cert: &'a [u8],
    oid: &[u8; 12],
    name: &str,
    len: usize,
) -> Result<&'a [u8], AgentError> {
    let at = cert
        .windows(oid.len())
        .position(|w| w == oid)
        .ok_or_else(|| AgentError::Quote(format!("PCK certificate has no {name}")))?;
    let value = at + oid.len();
    match cert.get(value..value + 2 + len) {
        Some([0x04, n, rest @ ..]) if *n as usize == len => Ok(rest),
        _ => Err(AgentError::Quote(format!(
            "PCK certificate {name} is malformed"
        ))),
    }
}

fn fmspc(cert: &[u8]) -> Result<[u8; 6], AgentError> {
    Ok(sgx_extension(cert, &FMSPC_OID, "FMSPC", 6)?
        .try_into()
        .unwrap())
}

/// The leaf's issuer names the CA; anything unrecognised is treated as the
/// processor CA, as dcap-qvl does.
fn pck_ca(cert: &[u8]) -> &'static str {
    let platform = b"Intel SGX PCK Platform CA";
    if cert.windows(platform.len()).any(|w| w == platform) {
        "platform"
    } else {
        "processor"
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FMSPC: [u8; 6] = [0x90, 0xc0, 0x6f, 0x00, 0x00, 0x00];
    pub(crate) const PPID: [u8; 16] = [0xab; 16];

    /// Bytes standing in for a DER leaf certificate: an issuer name and the SGX
    /// PPID and FMSPC extensions are all the parser looks at.
    fn fake_cert(issuer: &str) -> Vec<u8> {
        let mut der = vec![0x30, 0x82, 0x01, 0x00];
        der.extend_from_slice(issuer.as_bytes());
        der.extend_from_slice(&[0x30, 0x20]);
        der.extend_from_slice(&PPID_OID);
        der.extend_from_slice(&[0x04, 0x10]);
        der.extend_from_slice(&PPID);
        der.extend_from_slice(&[0x30, 0x10]);
        der.extend_from_slice(&FMSPC_OID);
        der.extend_from_slice(&[0x04, 0x06]);
        der.extend_from_slice(&FMSPC);
        der
    }

    fn pem(certs: &[Vec<u8>]) -> Vec<u8> {
        let mut out = String::new();
        for der in certs {
            let b64 = STANDARD.encode(der);
            out.push_str("-----BEGIN CERTIFICATE-----\n");
            for line in b64.as_bytes().chunks(64) {
                out.push_str(std::str::from_utf8(line).unwrap());
                out.push('\n');
            }
            out.push_str("-----END CERTIFICATE-----\n");
        }
        let mut bytes = out.into_bytes();
        // Tolerate trailing NUL padding after the chain.
        bytes.push(0);
        bytes
    }

    fn sized(out: &mut Vec<u8>, data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }

    fn header(version: u16, tee_type: u32) -> Vec<u8> {
        let mut h = vec![0u8; HEADER_LEN];
        h[..2].copy_from_slice(&version.to_le_bytes());
        h[4..8].copy_from_slice(&tee_type.to_le_bytes());
        h
    }

    /// A v4 TDX quote whose certification data is a QE report wrapping a PCK chain.
    pub(crate) fn tdx_quote(issuer: &str) -> Vec<u8> {
        quote_with(
            4,
            TDX_TEE_TYPE,
            true,
            &pem(&[fake_cert(issuer), vec![1, 2, 3]]),
        )
    }

    fn quote_with(version: u16, tee_type: u32, wrap_in_qe_report: bool, chain: &[u8]) -> Vec<u8> {
        let mut quote = header(version, tee_type);
        if version == 5 {
            quote.extend_from_slice(&2u16.to_le_bytes());
            sized(&mut quote, &[0u8; TD10_REPORT_LEN]);
        } else {
            quote.extend_from_slice(&[0u8; TD10_REPORT_LEN]);
        }

        let mut inner = CERT_DATA_PCK_CHAIN.to_le_bytes().to_vec();
        sized(&mut inner, chain);

        let mut sig = vec![0u8; 128];
        if wrap_in_qe_report {
            let mut qe = vec![0u8; QE_REPORT_LEN + 64];
            qe.extend_from_slice(&3u16.to_le_bytes());
            qe.extend_from_slice(&[7, 7, 7]);
            qe.extend_from_slice(&inner);
            sig.extend_from_slice(&CERT_DATA_QE_REPORT.to_le_bytes());
            sized(&mut sig, &qe);
        } else {
            sig.extend_from_slice(&inner);
        }
        sized(&mut quote, &sig);
        quote
    }

    #[test]
    fn reads_fmspc_and_platform_ca_from_v4_quote() {
        let info = pck_info(&tdx_quote("Intel SGX PCK Platform CA")).unwrap();
        assert_eq!(
            info,
            PckInfo {
                fmspc: "90C06F000000".into(),
                ca: "platform"
            }
        );
    }

    #[test]
    fn reads_the_ppid() {
        let quote = tdx_quote("Intel SGX PCK Platform CA");
        assert_eq!(ppid(&quote).unwrap(), "ab".repeat(16));

        let no_ppid = quote_with(4, TDX_TEE_TYPE, true, &pem(&[b"no extensions".to_vec()]));
        let err = ppid(&no_ppid).unwrap_err();
        assert!(err.to_string().contains("no PPID"), "{err}");
    }

    #[test]
    fn reads_v5_quotes_and_unwrapped_chains() {
        let chain = pem(&[fake_cert("Intel SGX PCK Processor CA")]);
        for (version, wrap) in [(5, true), (5, false), (4, false)] {
            let info = pck_info(&quote_with(version, TDX_TEE_TYPE, wrap, &chain)).unwrap();
            assert_eq!(info.ca, "processor");
            assert_eq!(info.fmspc, "90C06F000000");
        }
    }

    #[test]
    fn rejects_unsupported_quotes() {
        let chain = pem(&[fake_cert("x")]);
        let cases = [
            quote_with(4, 0, false, &chain),
            quote_with(3, TDX_TEE_TYPE, false, &chain),
            tdx_quote("x")[..HEADER_LEN + 10].to_vec(),
        ];
        for quote in cases {
            assert!(matches!(pck_info(&quote), Err(AgentError::Quote(_))));
        }

        // Certification data that isn't a PCK chain (e.g. encrypted PPID).
        let mut quote = header(4, TDX_TEE_TYPE);
        quote.extend_from_slice(&[0u8; TD10_REPORT_LEN]);
        let mut sig = vec![0u8; 128];
        sig.extend_from_slice(&3u16.to_le_bytes());
        sized(&mut sig, &[0u8; 16]);
        sized(&mut quote, &sig);
        let err = pck_info(&quote).unwrap_err();
        assert!(err.to_string().contains("certification data type 3"));
    }

    #[test]
    fn rejects_bad_certificates() {
        let no_cert = b"nothing here".to_vec();
        let unterminated = b"-----BEGIN CERTIFICATE-----\nAAAA".to_vec();
        let not_base64 = b"-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----".to_vec();
        let no_fmspc = pem(&[b"Intel SGX PCK Platform CA".to_vec()]);
        let mut truncated = fake_cert("x");
        truncated.truncate(truncated.len() - 3);
        let mut wrong_tag = fake_cert("x");
        let tag_at = wrong_tag.len() - 8;
        wrong_tag[tag_at] = 0x05;

        for chain in [
            no_cert,
            unterminated,
            not_base64,
            no_fmspc,
            pem(&[truncated]),
            pem(&[wrong_tag]),
        ] {
            let quote = quote_with(4, TDX_TEE_TYPE, true, &chain);
            assert!(matches!(pck_info(&quote), Err(AgentError::Quote(_))));
        }
    }
}

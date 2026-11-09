//! AWS's Signature Version 4 (host-capabilities L4 §5.9.9): what signs a request to SQS (sqs.rs),
//! and to S3 when step 1.4 comes. The canonical request (method, path, query, the headers signed,
//! the payload's SHA-256), the string to sign over its hash with the date and the scope
//! (`<date>/<region>/<service>/aws4_request`), and the signature: HMAC-SHA256 under a key derived
//! from the secret by the date, the region, the service and `aws4_request`, as AWS's documents
//! give it. On rustls's own ring, already in the build.

use ring::{digest, hmac};

/// The keys a request is signed with: an access key id, its secret, and a session token for
/// temporary ones.
#[derive(Clone)]
pub(crate) struct Keys {
    pub(crate) id: String,
    pub(crate) secret: zeroize::Zeroizing<String>,
    pub(crate) token: Option<zeroize::Zeroizing<String>>,
}

/// What a request is signed for: the region and the service (`sqs`).
pub(crate) struct Scope<'a> {
    pub(crate) region: &'a str,
    pub(crate) service: &'a str,
}

/// The SHA-256 of `bytes`, in lowercase hex.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn mac(key: &[u8], data: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data).as_ref().to_vec()
}

/// The canonical request of a request whose headers are `headers` (each name lowercase, sorted,
/// the host among them), with its payload's hash.
fn canonical(method: &str, path: &str, query: &str, headers: &[(String, String)], payload_hash: &str) -> String {
    let lines: String = headers.iter().map(|(n, v)| format!("{n}:{}\n", v.trim())).collect();
    let signed: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
    format!("{method}\n{path}\n{query}\n{lines}\n{}\n{payload_hash}", signed.join(";"))
}

/// The `Authorization` header of a request at `amz_date` (`20150830T123600Z`), its headers
/// `headers` as sent, the host among them: those, lowercased and sorted, are the ones signed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn authorization(keys: &Keys, scope: &Scope, amz_date: &str, method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload: &[u8]) -> String {
    let mut headers: Vec<(String, String)> = headers.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.to_string())).collect();
    headers.sort();
    let request = canonical(method, path, query, &headers, &sha256_hex(payload));
    let date = &amz_date[..8];
    let credential_scope = format!("{date}/{}/{}/aws4_request", scope.region, scope.service);
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}", sha256_hex(request.as_bytes()));
    let secret = zeroize::Zeroizing::new(format!("AWS4{}", keys.secret.as_str()));
    let key = [scope.region, scope.service, "aws4_request"].iter().fold(mac(secret.as_bytes(), date.as_bytes()), |k, part| mac(&k, part.as_bytes()));
    let signature = hex(&mac(&key, to_sign.as_bytes()));
    let signed: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
    format!("AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={}, Signature={signature}", keys.id, signed.join(";"))
}

/// Now, as a request's `X-Amz-Date` gives it: `20150830T123600Z`.
pub(crate) fn amz_date_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    amz_date(secs)
}

/// The time `secs` seconds since 1970, as `X-Amz-Date` gives it.
pub(crate) fn amz_date(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970 to a date (Howard Hinnant's civil_from_days).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS's own example (the IAM `ListUsers` request in "Create a signed AWS API request"): its
    /// signature, as AWS's documents print it.
    #[test]
    fn a_request_is_signed_as_aws_documents_it() {
        let keys = Keys { id: "AKIDEXAMPLE".into(), secret: zeroize::Zeroizing::new("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into()), token: None };
        let scope = Scope { region: "us-east-1", service: "iam" };
        let headers = [("Content-Type", "application/x-www-form-urlencoded; charset=utf-8"), ("Host", "iam.amazonaws.com"), ("X-Amz-Date", "20150830T123600Z")];
        let auth = authorization(&keys, &scope, "20150830T123600Z", "GET", "/", "Action=ListUsers&Version=2010-05-08", &headers, b"");
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn a_time_is_written_as_x_amz_date_writes_it() {
        assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(951_782_400), "20000229T000000Z");
    }
}

//! Authenticator codes: TOTP (RFC 6238) over HOTP (RFC 4226), SHA-1, SHA-256
//! or SHA-512, 6 to 8 digits, any period. Parses either a bare base32 setup
//! key or an `otpauth://totp/...` link.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TotpError {
    #[error("the authenticator key is empty")]
    Empty,
    #[error("the authenticator key is not valid (it should be a setup key or an otpauth:// link)")]
    Invalid,
    #[error("only time-based authenticator keys are supported")]
    NotTotp,
    #[error("the authenticator key is too short")]
    TooShort,
    #[error("the authenticator link asks for settings that are not supported")]
    Unsupported,
}

pub struct Totp {
    secret: Zeroizing<Vec<u8>>,
    pub algorithm: Algorithm,
    pub digits: u32,
    pub period: u64,
}

impl std::fmt::Debug for Totp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Totp").field("digits", &self.digits).field("period", &self.period).finish_non_exhaustive()
    }
}

fn base32_decode(s: &str) -> Option<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::new());
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '=') {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Totp {
    pub fn from_secret(secret: Vec<u8>, algorithm: Algorithm, digits: u32, period: u64) -> Result<Self, TotpError> {
        if !(6..=8).contains(&digits) || period == 0 || period > 600 {
            return Err(TotpError::Unsupported);
        }
        if secret.len() < 10 {
            return Err(TotpError::TooShort);
        }
        Ok(Totp { secret: Zeroizing::new(secret), algorithm, digits, period })
    }

    /// A setup key or an `otpauth://totp/` link.
    pub fn parse(input: &str) -> Result<Self, TotpError> {
        let t = input.trim();
        if t.is_empty() {
            return Err(TotpError::Empty);
        }
        if !t.to_ascii_lowercase().starts_with("otpauth://") {
            let secret = base32_decode(t).ok_or(TotpError::Invalid)?;
            return Self::from_secret(secret.to_vec(), Algorithm::Sha1, 6, 30);
        }
        let url = url::Url::parse(t).map_err(|_| TotpError::Invalid)?;
        if url.host_str().map(str::to_ascii_lowercase).as_deref() != Some("totp") {
            return Err(TotpError::NotTotp);
        }
        let (mut secret, mut algo, mut digits, mut period) = (None, Algorithm::Sha1, 6u32, 30u64);
        for (k, v) in url.query_pairs() {
            match k.to_ascii_lowercase().as_str() {
                "secret" => secret = Some(base32_decode(&v).ok_or(TotpError::Invalid)?),
                "algorithm" => {
                    algo = match v.to_ascii_uppercase().as_str() {
                        "SHA1" => Algorithm::Sha1,
                        "SHA256" => Algorithm::Sha256,
                        "SHA512" => Algorithm::Sha512,
                        _ => return Err(TotpError::Unsupported),
                    }
                }
                "digits" => digits = v.parse().map_err(|_| TotpError::Unsupported)?,
                "period" => period = v.parse().map_err(|_| TotpError::Unsupported)?,
                _ => {}
            }
        }
        let secret = secret.ok_or(TotpError::Invalid)?;
        Self::from_secret(secret.to_vec(), algo, digits, period)
    }

    fn hmac(&self, msg: &[u8; 8]) -> Zeroizing<Vec<u8>> {
        macro_rules! run {
            ($h:ty) => {{
                let mut m = <Hmac<$h> as Mac>::new_from_slice(&self.secret).expect("HMAC takes any key length");
                m.update(msg);
                Zeroizing::new(m.finalize().into_bytes().to_vec())
            }};
        }
        match self.algorithm {
            Algorithm::Sha1 => run!(Sha1),
            Algorithm::Sha256 => run!(Sha256),
            Algorithm::Sha512 => run!(Sha512),
        }
    }

    /// The code for the 30 s (or other) step that contains `unix_secs`.
    pub fn code_at(&self, unix_secs: u64) -> Zeroizing<String> {
        let counter = (unix_secs / self.period).to_be_bytes();
        let h = self.hmac(&counter);
        let off = (h[h.len() - 1] & 0x0f) as usize;
        let bin = u32::from_be_bytes([h[off] & 0x7f, h[off + 1], h[off + 2], h[off + 3]]);
        let code = bin % 10u32.pow(self.digits);
        Zeroizing::new(format!("{code:0width$}", width = self.digits as usize))
    }

    /// Seconds until the current code stops being valid.
    pub fn seconds_left(&self, unix_secs: u64) -> u64 {
        self.period - unix_secs % self.period
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B: the seeds are ASCII strings of 20, 32 and 64 bytes.
    const SEED1: &[u8] = b"12345678901234567890";
    const SEED256: &[u8] = b"12345678901234567890123456789012";
    const SEED512: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";
    const TIMES: [u64; 6] = [59, 1111111109, 1111111111, 1234567890, 2000000000, 20000000000];

    fn check(seed: &[u8], algo: Algorithm, want: [&str; 6]) {
        let t = Totp::from_secret(seed.to_vec(), algo, 8, 30).unwrap();
        for (time, w) in TIMES.iter().zip(want) {
            assert_eq!(&*t.code_at(*time), w, "{algo:?} at {time}");
        }
    }

    #[test]
    fn rfc6238_sha1() {
        check(SEED1, Algorithm::Sha1, ["94287082", "07081804", "14050471", "89005924", "69279037", "65353130"]);
    }

    #[test]
    fn rfc6238_sha256() {
        check(SEED256, Algorithm::Sha256, ["46119246", "68084774", "67062674", "91819424", "90698825", "77737706"]);
    }

    #[test]
    fn rfc6238_sha512() {
        check(SEED512, Algorithm::Sha512, ["90693936", "25091201", "99943326", "93441116", "38618901", "47863826"]);
    }

    #[test]
    fn rfc4226_hotp_counter_vectors() {
        // RFC 4226 appendix D, through a 1-second period so counter = time.
        let t = Totp::from_secret(SEED1.to_vec(), Algorithm::Sha1, 6, 1).unwrap();
        let want = ["755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871", "520489"];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(&*t.code_at(i as u64), w, "counter {i}");
        }
    }

    #[test]
    fn six_digit_code_is_zero_padded() {
        let t = Totp::from_secret(SEED1.to_vec(), Algorithm::Sha1, 6, 30).unwrap();
        // 1111111109 gives 07081804 with 8 digits, so 081804 with 6.
        assert_eq!(&*t.code_at(1111111109), "081804");
    }

    #[test]
    fn base32_key_and_link() {
        // "12345678901234567890" in base32.
        let key = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let a = Totp::parse(key).unwrap();
        assert_eq!(&*a.code_at(59), &Totp::from_secret(SEED1.to_vec(), Algorithm::Sha1, 6, 30).unwrap().code_at(59)[..]);
        let spaced = Totp::parse("gezd gnbv-gy3t qojq gezd gnbv gy3t qojq").unwrap();
        assert_eq!(&*spaced.code_at(59), &*a.code_at(59));
        let link = format!("otpauth://totp/Example:me@example.com?secret={key}&issuer=Example&algorithm=SHA256&digits=8&period=60");
        let b = Totp::parse(&link).unwrap();
        assert_eq!((b.algorithm, b.digits, b.period), (Algorithm::Sha256, 8, 60));
        assert_eq!(b.seconds_left(61), 59);
        // Defaults when the link omits them.
        let c = Totp::parse(&format!("otpauth://totp/x?secret={key}")).unwrap();
        assert_eq!((c.algorithm, c.digits, c.period), (Algorithm::Sha1, 6, 30));
    }

    #[test]
    fn bad_keys_are_refused() {
        assert_eq!(Totp::parse("").unwrap_err(), TotpError::Empty);
        assert_eq!(Totp::parse("not base32 !!!").unwrap_err(), TotpError::Invalid);
        assert_eq!(Totp::parse("ABCD").unwrap_err(), TotpError::TooShort);
        assert_eq!(Totp::parse("otpauth://hotp/x?secret=GEZDGNBVGY3TQOJQ&counter=1").unwrap_err(), TotpError::NotTotp);
        assert_eq!(Totp::parse("otpauth://totp/x").unwrap_err(), TotpError::Invalid);
        assert_eq!(Totp::parse("otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ&digits=12").unwrap_err(), TotpError::Unsupported);
        assert_eq!(Totp::parse("otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ&algorithm=MD5").unwrap_err(), TotpError::Unsupported);
        assert_eq!(Totp::parse("otpauth://totp/x?secret=GEZDGNBVGY3TQOJQ&period=0").unwrap_err(), TotpError::Unsupported);
    }

    #[test]
    fn debug_hides_the_secret() {
        let t = Totp::from_secret(SEED1.to_vec(), Algorithm::Sha1, 6, 30).unwrap();
        assert!(!format!("{t:?}").contains("1234"));
    }
}

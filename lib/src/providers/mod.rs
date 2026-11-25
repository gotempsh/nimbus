pub mod digitalocean;
pub mod hetzner;
pub mod hostinger;
pub mod linode;
pub mod ovh;
pub mod scaleway;
pub mod vultr;

pub use digitalocean::DigitalOcean;
pub use hetzner::Hetzner;
pub use hostinger::Hostinger;
pub use linode::Linode;
pub use ovh::{Ovh, OvhRegion};
pub use scaleway::Scaleway;
pub use vultr::Vultr;

/// Standard-alphabet, padded base64 (RFC 4648 §4). Several provider APIs
/// (Vultr, Linode) only accept instance user data in this form; kept local
/// rather than pulling in a dependency for one small function.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sextet = |n: u32, shift: u32| TBL[((n >> shift) & 63) as usize] as char;
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(sextet(n, 18));
        out.push(sextet(n, 12));
        out.push(if chunk.len() > 1 { sextet(n, 6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(n, 0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(input.as_bytes()), expected, "input {input:?}");
        }
    }

    #[test]
    fn base64_handles_non_ascii_and_high_bytes() {
        assert_eq!(base64_encode(&[0xff, 0xfe, 0xfd]), "//79");
        assert_eq!(base64_encode("é\n".as_bytes()), "w6kK");
    }
}

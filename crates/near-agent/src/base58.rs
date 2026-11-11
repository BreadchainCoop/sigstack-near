//! Bitcoin-alphabet base58, as NEAR uses for keys and hashes.

const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

pub fn encode(input: &[u8]) -> String {
    let zeros = input.iter().take_while(|&&b| b == 0).count();
    // Base58 digits, least significant first.
    let mut digits: Vec<u8> = Vec::with_capacity(input.len() * 138 / 100 + 1);
    for &byte in &input[zeros..] {
        let mut carry = byte as u32;
        for d in digits.iter_mut() {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut out = "1".repeat(zeros);
    out.extend(digits.iter().rev().map(|&d| ALPHABET[d as usize] as char));
    out
}

/// Returns `None` if `input` has a character outside the alphabet.
pub fn decode(input: &str) -> Option<Vec<u8>> {
    let zeros = input.bytes().take_while(|&b| b == b'1').count();
    // Bytes, least significant first.
    let mut bytes: Vec<u8> = Vec::with_capacity(input.len());
    for c in input.bytes().skip(zeros) {
        let mut carry = ALPHABET.iter().position(|&a| a == c)? as u32;
        for b in bytes.iter_mut() {
            carry += (*b as u32) * 58;
            *b = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let mut out = vec![0u8; zeros];
    out.extend(bytes.iter().rev());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(encode(b"Hello World!"), "2NEpo7TZRRrLZSi2U");
        assert_eq!(encode(&[0x10, 0xc8, 0x51, 0x1e]), "Rt5zm");
        assert_eq!(encode(&[0, 0, 0x10, 0xc8, 0x51, 0x1e]), "11Rt5zm");
        assert_eq!(encode(&[]), "");
        assert_eq!(encode(&[0, 0]), "11");
        assert_eq!(decode("2NEpo7TZRRrLZSi2U").unwrap(), b"Hello World!");
        assert_eq!(decode("11Rt5zm").unwrap(), [0, 0, 0x10, 0xc8, 0x51, 0x1e]);
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn rejects_characters_outside_alphabet() {
        assert!(decode("0").is_none());
        assert!(decode("abcO").is_none());
        assert!(decode("l").is_none());
    }

    #[test]
    fn round_trips_32_byte_values() {
        for seed in 0u8..=40 {
            let bytes: Vec<u8> = (0..32u8)
                .map(|i| i.wrapping_mul(seed).wrapping_add(seed))
                .collect();
            assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
        }
    }
}

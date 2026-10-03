/// Deterministic 64-bit FNV-1a hash implementation (zero dependencies).
/// Guarantees consistent hash values across platforms, architectures, and Rust versions.
pub fn fnv1a_u64(input: &[u8]) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for &byte in input {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

/// Computes the 16-character hexadecimal representation of the FNV-1a hash of a string slice.
pub fn fnv1a_hex(input: &str) -> String {
    format!("{:016x}", fnv1a_u64(input.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_empty_string() {
        assert_eq!(fnv1a_u64(b""), 14695981039346656037);
        assert_eq!(fnv1a_hex(""), "cbf29ce484222325");
    }

    #[test]
    fn test_fnv1a_known_values() {
        let h1 = fnv1a_hex("/home/user/Music");
        let h2 = fnv1a_hex("/home/user/Music");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);

        // Different strings must yield different hashes
        let h3 = fnv1a_hex("/home/user/Music2");
        assert_ne!(h1, h3);
    }
}

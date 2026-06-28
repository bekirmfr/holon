/// BLAKE3 over a byte slice — the primitive the canonical hashing pass is built on.
pub fn hash_bytes(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_is_deterministic() {
        assert_eq!(hash_bytes(b"holon"), hash_bytes(b"holon"));
        assert_ne!(hash_bytes(b"holon"), hash_bytes(b"holons"));
    }
}
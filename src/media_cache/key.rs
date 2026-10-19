use sha2::{Digest, Sha256};

const KEY_LEN: usize = 16;

/// Truncated sha256 of "<extractor>:<id>"; all URL variants share it.
pub fn canonical_key(extractor: &str, id: &str) -> String {
    hash(&format!("{extractor}:{id}"))
}

fn hash(input: &str) -> String {
    let digest = Sha256::new().chain_update(input.as_bytes()).finalize();
    hex(&digest)[..KEY_LEN].to_string()
}

pub(crate) fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable_and_truncated() {
        assert_eq!(
            canonical_key("youtube", "abc"),
            canonical_key("youtube", "abc")
        );
        assert_eq!(canonical_key("youtube", "abc").len(), 16);
    }

    #[test]
    fn keys_differ_between_extractors_and_ids() {
        assert_ne!(
            canonical_key("youtube", "abc"),
            canonical_key("vimeo", "abc")
        );
        assert_ne!(
            canonical_key("youtube", "abc"),
            canonical_key("youtube", "xyz")
        );
    }
}

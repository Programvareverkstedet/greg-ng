use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Persisted as `meta.json`; the full yt-dlp dump sits alongside as `*.info.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaEntry {
    pub schema_version: u32,
    pub key: String,
    pub urls: Vec<String>,
    pub title: Option<String>,
    pub media_file: String,
    pub size: u64,
    pub content_hash: Option<String>,
    pub created_at: u64,
    pub last_used: u64,
}

impl MetaEntry {
    pub fn record_use(&mut self, now: u64) {
        self.last_used = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MetaEntry {
        MetaEntry {
            schema_version: SCHEMA_VERSION,
            key: "0123456789abcdef".to_string(),
            urls: vec!["https://example.com/w1".to_string()],
            title: Some("Test".to_string()),
            media_file: "video.mkv".to_string(),
            size: 1024,
            content_hash: Some("deadbeef".to_string()),
            created_at: 100,
            last_used: 100,
        }
    }

    #[test]
    fn meta_entry_roundtrips_through_json() {
        let meta = sample();
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: MetaEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.key, meta.key);
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert_eq!(parsed.media_file, "video.mkv");
    }

    #[test]
    fn record_use_updates_last_used() {
        let mut meta = sample();
        meta.record_use(101);
        assert_eq!(meta.last_used, 101);
    }
}

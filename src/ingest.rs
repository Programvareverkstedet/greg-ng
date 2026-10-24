use std::{
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures::{Stream, StreamExt, stream};
use mpvipc_async::{Mpv, MpvExt, PlaylistAddOptions, PlaylistAddTypeOptions};
use tokio::sync::mpsc;

use crate::util::{ExpandedEntry, expand_entries};

const CHANNEL_CAPACITY: usize = 64;

/// Number of URLs the pipeline expands concurrently
const EXPANSION_CONCURRENCY: usize = 4;

/// Cap on how many entries a single URL expands into. Since expansion is
/// single-pass, this applies to fixed-size playlists too, not just mixes.
const MAX_ENTRIES: u64 = 200;

/// FIFO queue that expands playlist URLs and inserts their entries into
/// mpv with pre-fetched titles. Up to `EXPANSION_CONCURRENCY` URLs expand
/// at a time; insertion into mpv is sequential, and entries from the same
/// URL keep their order. URLs whose expansion fails are inserted as-is.
#[derive(Debug, Clone)]
pub struct IngestQueue {
    mpv: Mpv,
    tx: mpsc::Sender<(String, u64)>,
    generation: Arc<AtomicU64>,
}

impl IngestQueue {
    pub fn new(mpv: Mpv, cookies_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let generation = Arc::new(AtomicU64::new(0));

        tokio::spawn({
            let mpv = mpv.clone();
            let generation = generation.clone();
            async move {
                let pipeline = stream::unfold(rx, |mut rx| async {
                    rx.recv().await.map(|item| (item, rx))
                })
                .map(|(url, url_generation)| {
                    expand_url(
                        url,
                        url_generation,
                        cookies_path.clone(),
                        generation.clone(),
                    )
                })
                .flatten_unordered(EXPANSION_CONCURRENCY);
                let mut pipeline = std::pin::pin!(pipeline);

                while let Some(entry) = pipeline.next().await {
                    if entry.generation == generation.load(Ordering::SeqCst)
                        && let Err(e) = insert_entry(&mpv, &entry.url, entry.title.as_deref()).await
                    {
                        tracing::warn!("Failed to insert {:?}: {e:#}", entry.url);
                    }
                }
            }
        });

        Self {
            mpv,
            tx,
            generation,
        }
    }

    /// Queue a URL for expansion and insertion
    pub async fn enqueue(&self, url: String) {
        let generation = self.generation.load(Ordering::SeqCst);
        if self.tx.send((url, generation)).await.is_err() {
            tracing::warn!("Ingest queue is closed");
        }
    }

    /// Clear the mpv playlist, and stop in-flight expansions from
    /// producing more entries
    pub async fn playlist_clear(&self) -> anyhow::Result<()> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.mpv.playlist_clear().await.map_err(anyhow::Error::from)
    }
}

struct Entry {
    generation: u64,
    url: String,
    /// Without one, mpv resolves the title at play time
    title: Option<String>,
}

enum ExpandUrl {
    Start {
        url: String,
    },
    Streaming {
        url: String,
        entries: Pin<Box<dyn Stream<Item = anyhow::Result<ExpandedEntry>> + Send>>,
        emitted: bool,
    },
    Done,
}

/// Stream of entries a queued URL expands into, ending with the raw URL
/// itself when nothing was expanded
fn expand_url(
    url: String,
    url_generation: u64,
    cookies_path: PathBuf,
    generation: Arc<AtomicU64>,
) -> Pin<Box<dyn Stream<Item = Entry> + Send>> {
    Box::pin(stream::unfold(ExpandUrl::Start { url }, move |state| {
        let cookies_path = cookies_path.clone();
        let generation = generation.clone();
        async move {
            let mut state = state;
            loop {
                match std::mem::replace(&mut state, ExpandUrl::Done) {
                    ExpandUrl::Done => return None,

                    ExpandUrl::Start { url } => {
                        if url_generation != generation.load(Ordering::SeqCst) {
                            return None;
                        }
                        if !is_remote(&url) {
                            return Some((
                                Entry {
                                    generation: url_generation,
                                    url,
                                    title: None,
                                },
                                ExpandUrl::Done,
                            ));
                        }
                        // Entries stream in as yt-dlp paginates
                        let entries = Box::pin(expand_entries(
                            url.clone(),
                            cookies_path.clone(),
                            MAX_ENTRIES,
                        ));
                        state = ExpandUrl::Streaming {
                            url,
                            entries,
                            emitted: false,
                        };
                    }

                    ExpandUrl::Streaming {
                        url,
                        mut entries,
                        emitted,
                    } => {
                        if url_generation != generation.load(Ordering::SeqCst) {
                            // Dropping the entries stream kills the yt-dlp process
                            return None;
                        }

                        match entries.next().await {
                            Some(Ok(expanded)) => {
                                return Some((
                                    Entry {
                                        generation: url_generation,
                                        url: expanded.url,
                                        title: expanded.title,
                                    },
                                    ExpandUrl::Streaming {
                                        url,
                                        entries,
                                        emitted: true,
                                    },
                                ));
                            }
                            Some(Err(e)) => {
                                tracing::warn!("Expansion failed for {url:?}: {e:#}");
                            }
                            None => {}
                        }

                        if emitted {
                            return None;
                        }
                        return Some((
                            Entry {
                                generation: url_generation,
                                url,
                                title: None,
                            },
                            ExpandUrl::Done,
                        ));
                    }
                }
            }
        }
    }))
}

async fn insert_entry(mpv: &Mpv, url: &str, title: Option<&str>) -> anyhow::Result<()> {
    mpv.playlist_add(
        url,
        PlaylistAddTypeOptions::File,
        PlaylistAddOptions::Append,
    )
    .await
    .map_err(anyhow::Error::from)?;

    if let Some(title) = title {
        let tagged = async {
            let index = mpv.get_playlist().await?.0.len().saturating_sub(1);
            mpv.set_property(format!("playlist/{index}/title").as_str(), title)
                .await
        }
        .await;
        if let Err(e) = tagged {
            tracing::warn!("Inserted {url:?} without a title: {e}");
        }
    }

    Ok(())
}

fn is_remote(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls_are_classified() {
        assert!(is_remote("http://example.com"));
        assert!(is_remote("https://example.com"));
        assert!(!is_remote("file:///tmp/video.mkv"));
        assert!(!is_remote("/tmp/video.mkv"));
        assert!(!is_remote("magnet:?xt=urn:btih:..."));
    }
}

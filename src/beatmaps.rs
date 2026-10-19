use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use akatsuki_pp_rs::Beatmap;
use futures::future::{BoxFuture, FutureExt, Shared};

const DEFAULT_BASE_URL: &str = "https://old.ppy.sh/osu";

type Download = Shared<BoxFuture<'static, Result<(), Arc<str>>>>;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("static client configuration is valid")
    })
}

fn in_flight() -> &'static Mutex<HashMap<i32, Download>> {
    static IN_FLIGHT: OnceLock<Mutex<HashMap<i32, Download>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}

// Fetches the .osu into the cache unless a usable copy is already there. An empty file is never kept: every later
// calculation on the map would read it and give 0pp, and nothing would fetch it again.
//
// Callers asking for the same map while it is being fetched share that one fetch, and its outcome. It runs as its own
// task so a caller that goes away (client disconnect) doesn't cancel it for the others.
pub async fn ensure_cached(path: &Path, beatmap_id: i32) -> anyhow::Result<()> {
    if tokio::fs::metadata(path).await.is_ok_and(|meta| meta.len() > 0) {
        return Ok(());
    }

    let download = {
        let mut map = in_flight().lock().unwrap();
        map.entry(beatmap_id)
            .or_insert_with(|| {
                let path = path.to_owned();
                let task = tokio::spawn(async move {
                    let result = fetch(&path, beatmap_id).await.map_err(|e| Arc::<str>::from(format!("{e:#}")));
                    in_flight().lock().unwrap().remove(&beatmap_id);
                    result
                });
                async move { task.await.unwrap_or_else(|e| Err(Arc::from(format!("download task failed: {e}")))) }
                    .boxed()
                    .shared()
            })
            .clone()
    };

    download.await.map_err(|e| anyhow::anyhow!("{e}"))
}

async fn fetch(path: &Path, beatmap_id: i32) -> anyhow::Result<()> {
    // Another process, or a fetch that finished between the caller's check and ours, may have put it there already.
    if tokio::fs::metadata(path).await.is_ok_and(|meta| meta.len() > 0) {
        return Ok(());
    }

    let base = std::env::var("OSU_BEATMAP_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());
    log::info!("downloading .osu for beatmap {beatmap_id}");
    let bytes = client()
        .get(format!("{base}/{beatmap_id}"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if bytes.is_empty() {
        anyhow::bail!("old.ppy.sh sent an empty .osu for beatmap {beatmap_id}");
    }

    // Written under a name of its own and moved into place, so neither a crash mid-write nor another process
    // fetching the same map can leave a cut-off or interleaved file behind.
    let partial = path.with_extension(format!("osu.{}.part", uuid::Uuid::new_v4().simple()));
    let written = tokio::fs::write(&partial, &bytes).await;
    let moved = match written {
        Ok(()) => tokio::fs::rename(&partial, path).await,
        Err(e) => Err(e),
    };
    if let Err(e) = moved {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e.into());
    }
    Ok(())
}

// Parsed maps are what every calculation starts from and parsing is a large share of the work, so the most recently
// used ones are kept. An entry is only reused while the file's modified time and size are unchanged. The byte budget
// counts the .osu sizes, which is the most the parsed form can come to.
const CACHE_ENTRIES: usize = 256;
const CACHE_BYTES: u64 = 128 << 20;

struct CachedMap {
    modified: Option<SystemTime>,
    len: u64,
    map: Arc<Beatmap>,
    used: u64,
}

#[derive(Default)]
struct ParsedCache {
    entries: HashMap<i32, CachedMap>,
    bytes: u64,
    clock: u64,
}

fn parsed_cache() -> &'static Mutex<ParsedCache> {
    static CACHE: OnceLock<Mutex<ParsedCache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

// Blocking: reads and parses the .osu unless an up-to-date parse is cached.
pub fn parsed(path: &Path, beatmap_id: i32) -> anyhow::Result<Arc<Beatmap>> {
    let meta = std::fs::metadata(path)?;
    let (modified, len) = (meta.modified().ok(), meta.len());

    {
        let mut cache = parsed_cache().lock().unwrap();
        cache.clock += 1;
        let clock = cache.clock;
        if let Some(entry) = cache.entries.get_mut(&beatmap_id) {
            if entry.modified == modified && entry.len == len {
                entry.used = clock;
                return Ok(entry.map.clone());
            }
        }
    }

    let map = Arc::new(Beatmap::from_path(path).map_err(|e| anyhow::anyhow!("{e}"))?);

    let mut cache = parsed_cache().lock().unwrap();
    cache.clock += 1;
    let used = cache.clock;
    if let Some(old) = cache.entries.insert(beatmap_id, CachedMap { modified, len, map: map.clone(), used }) {
        cache.bytes -= old.len;
    }
    cache.bytes += len;
    while cache.entries.len() > CACHE_ENTRIES || cache.bytes > CACHE_BYTES {
        let Some(oldest) = cache.entries.iter().filter(|(id, _)| **id != beatmap_id).min_by_key(|(_, e)| e.used).map(|(id, _)| *id)
        else {
            break;
        };
        if let Some(evicted) = cache.entries.remove(&oldest) {
            cache.bytes -= evicted.len;
        }
    }
    Ok(map)
}

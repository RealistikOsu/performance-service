use std::path::Path;

// Fetches the .osu into the cache unless a usable copy is already there. An empty file is never kept: every later
// calculation on the map would read it and give 0pp, and nothing would fetch it again.
pub async fn ensure_cached(path: &Path, beatmap_id: i32) -> anyhow::Result<()> {
    if tokio::fs::metadata(path).await.is_ok_and(|meta| meta.len() > 0) {
        return Ok(());
    }

    let bytes = reqwest::get(format!("https://old.ppy.sh/osu/{beatmap_id}"))
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if bytes.is_empty() {
        anyhow::bail!("old.ppy.sh sent an empty .osu for beatmap {beatmap_id}");
    }

    // Written aside and moved into place, so a crash mid-write can't leave a cut-off file behind.
    let partial = path.with_extension("osu.part");
    tokio::fs::write(&partial, &bytes).await?;
    tokio::fs::rename(&partial, path).await?;
    Ok(())
}

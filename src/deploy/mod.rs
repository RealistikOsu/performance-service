use crate::{
    api::routes::calculate::{calculate, CalculateRequest as LazerCalculateRequest},
    context::Context,
    models::score::RippleScore,
};
use akatsuki_pp_rs::{model::mode::GameMode, Beatmap};
use redis::AsyncCommands;
use std::{
    collections::HashMap,
    hash::Hash,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

use std::io::Write;
use tokio::fs::File;
use tokio::sync::Mutex;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CalculateRequest {
    pub beatmap_id: i32,
    pub mode: i32,
    pub mods: i32,
    pub max_combo: i32,
    pub accuracy: f32,
    pub miss_count: i32,
    pub playback_rate: f32,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CalculateResponse {
    pub stars: f32,
    pub pp: f32,
}

fn round(x: f32, decimals: u32) -> f32 {
    let y = 10i32.pow(decimals) as f32;
    (x * y).round() / y
}

const RX: i32 = 1 << 7;
const AP: i32 = 1 << 13;

async fn calculate_special_pp(
    beatmap_path: PathBuf,
    request: &CalculateRequest,
    recalc_ctx: &Arc<Mutex<RecalculateContext>>,
) -> CalculateResponse {
    let mut recalc_mutex = recalc_ctx.lock().await;

    let beatmap = if recalc_mutex.beatmaps.contains_key(&request.beatmap_id) {
        recalc_mutex
            .beatmaps
            .get(&request.beatmap_id)
            .unwrap()
            .clone()
    } else {
        match Beatmap::from_path(beatmap_path) {
            Ok(beatmap) => {
                recalc_mutex
                    .beatmaps
                    .insert(request.beatmap_id, beatmap.clone());

                beatmap
            }
            Err(_) => {
                return CalculateResponse {
                    stars: 0.0,
                    pp: 0.0,
                }
            }
        }
    };

    drop(recalc_mutex);

    let result = akatsuki_pp_rs::osu_2019::OsuPP::from_map(&beatmap)
        .mods(request.mods as u32)
        .combo(request.max_combo as u32)
        .misses(request.miss_count as u32)
        .accuracy(request.accuracy)
        .clock_rate(request.playback_rate as f64)
        .calculate();

    let mut pp = round(result.pp as f32, 2);
    if pp.is_infinite() || pp.is_nan() {
        pp = 0.0;
    }

    let mut stars = round(result.difficulty.stars as f32, 2);
    if stars.is_infinite() || stars.is_nan() {
        stars = 0.0;
    }

    CalculateResponse { stars, pp }
}

async fn calculate_rosu_pp(
    beatmap_path: PathBuf,
    request: &CalculateRequest,
    recalc_ctx: &Arc<Mutex<RecalculateContext>>,
) -> CalculateResponse {
    let mut recalc_mutex = recalc_ctx.lock().await;

    let beatmap = if recalc_mutex.beatmaps.contains_key(&request.beatmap_id) {
        recalc_mutex
            .beatmaps
            .get(&request.beatmap_id)
            .unwrap()
            .clone()
    } else {
        match Beatmap::from_path(beatmap_path) {
            Ok(beatmap) => {
                recalc_mutex
                    .beatmaps
                    .insert(request.beatmap_id, beatmap.clone());

                beatmap
            }
            Err(_) => {
                return CalculateResponse {
                    stars: 0.0,
                    pp: 0.0,
                }
            }
        }
    };

    drop(recalc_mutex);

    let result = beatmap
        .performance()
        .try_mode(match request.mode {
            0 => GameMode::Osu,
            1 => GameMode::Taiko,
            2 => GameMode::Catch,
            3 => GameMode::Mania,
            _ => unreachable!(),
        })
        .unwrap()
        .lazer(false)
        .mods(request.mods as u32)
        .combo(request.max_combo as u32)
        .accuracy(request.accuracy as f64)
        .misses(request.miss_count as u32)
        .clock_rate(request.playback_rate as f64)
        .calculate();

    let mut pp = round(result.pp() as f32, 2);
    if pp.is_infinite() || pp.is_nan() {
        pp = 0.0;
    }

    let mut stars = round(result.stars() as f32, 2);
    if stars.is_infinite() || stars.is_nan() {
        stars = 0.0;
    }

    CalculateResponse { stars, pp }
}

async fn recalculate_score(
    score: RippleScore,
    beatmap_path: PathBuf,
    ctx: Arc<Context>,
    recalc_ctx: Arc<Mutex<RecalculateContext>>,
) -> anyhow::Result<()> {
    let request = CalculateRequest {
        beatmap_id: score.beatmap_id,
        mode: score.play_mode,
        mods: score.mods,
        max_combo: score.max_combo,
        accuracy: score.accuracy,
        miss_count: score.count_misses,
        playback_rate: score.playback_rate as f32,
    };

    let response = if score.mods & RX > 0 && score.play_mode == 0 {
        calculate_special_pp(beatmap_path, &request, &recalc_ctx).await
    } else {
        calculate_rosu_pp(beatmap_path, &request, &recalc_ctx).await
    };

    let rx = if score.mods & RX > 0 {
        1
    } else if score.mods & AP > 0 {
        2
    } else {
        0
    };

    let scores_table = match rx {
        0 => "scores",
        1 => "scores_relax",
        2 => "scores_ap",
        _ => unreachable!(),
    };

    sqlx::query(&format!("UPDATE {} SET pp = ? WHERE id = ?", scores_table))
        .bind(response.pp)
        .bind(score.id)
        .execute(&ctx.database)
        .await?;

    // cache will only contain it if it's their best score
    if score.completed == 3 {
        let mut redis_connection = ctx.redis.get_async_connection().await?;
        redis_connection
            .publish(
                "cache:update_score_pp",
                serde_json::json!({
                    "beatmap_id": score.beatmap_id,
                    "user_id": score.userid,
                    "score_id": score.id,
                    "new_pp": response.pp,
                    "mode_vn": score.play_mode,
                    "relax": rx,
                })
                .to_string(),
            )
            .await?;
    }

    log::info!(
        "Recalculated score ID {} (mode: {}) | {} -> {}",
        score.id,
        score.play_mode,
        score.pp,
        response.pp,
    );

    Ok(())
}

async fn recalculate_mode_scores(
    mode: i32,
    rx: i32,
    ctx: Arc<Context>,
    recalc_ctx: Arc<Mutex<RecalculateContext>>,
) -> anyhow::Result<()> {
    let scores_table = match rx {
        0 => "scores",
        1 => "scores_relax",
        2 => "scores_ap",
        _ => unreachable!(),
    };

    let scores: Vec<RippleScore> = sqlx::query_as(
        &format!(
            "SELECT s.id, s.beatmap_md5, s.userid, s.score, s.max_combo, s.full_combo, s.mods, (s.playback_rate + 0e0) AS playback_rate, s.300_count,
            s.100_count, s.50_count, s.katus_count, s.gekis_count, s.misses_count, s.time, s.play_mode, s.completed,
            s.accuracy, s.pp, b.beatmap_id, b.beatmapset_id
            FROM {} s
            INNER JOIN
                beatmaps b
                USING(beatmap_md5)
            WHERE
                completed IN (2, 3)
                AND play_mode = ?
            ORDER BY pp DESC",
            scores_table
        )
    )
    .bind(mode)
    .fetch_all(&ctx.database)
    .await?;

    for score_chunk in scores.chunks(100).map(|c| c.to_vec()) {
        let mut futures = Vec::new();

        for score in score_chunk {
            let beatmap_path =
                Path::new(&ctx.config.beatmaps_path).join(format!("{}.osu", score.beatmap_id));

            if !beatmap_path.exists() {
                log::info!(
                    "Beatmap {} doesn't exist, fetching from bancho",
                    score.beatmap_id
                );

                let response =
                    reqwest::get(&format!("https://old.ppy.sh/osu/{}", score.beatmap_id))
                        .await?
                        .error_for_status();

                if response.is_err() {
                    log::warn!("Failed to get .osu for beatmap {}", score.beatmap_id);
                }

                let resp = response.unwrap();

                let mut file = File::create(&beatmap_path).await?;
                let mut content = Cursor::new(resp.bytes().await?);
                tokio::io::copy(&mut content, &mut file).await?;
            }

            let future = tokio::spawn(recalculate_score(
                score,
                beatmap_path,
                ctx.clone(),
                recalc_ctx.clone(),
            ));
            futures.push(future);
        }

        futures::future::try_join_all(futures).await?;
    }

    Ok(())
}

#[derive(sqlx::FromRow)]
struct LazerScoreRow {
    id: i64,
    beatmap_id: i32,
    accuracy: f64,
    max_combo: i32,
    mods: String,
    statistics: String,
    pp: f32,
    ranked_mods: bool,
}

const LAZER_ANY_SETTINGS: [&str; 17] = [
    "AC", "AL", "BL", "CO", "MU", "NS", "PF", "SD", "SG", "SW", "TC", "DT", "HT", "NC", "DC", "RX",
    "AP",
];
const LAZER_DEFAULT_SETTINGS: [&str; 13] = [
    "EZ", "HD", "HR", "FL", "NF", "SO", "TD", "4K", "5K", "6K", "7K", "8K", "9K",
];

/// Mirrors the lazer server's `PpMods`: a play counts only when every mod is one lazer ranks (kept with default settings
/// unless it's a rate change), or RX/AP.
fn lazer_mods_ranked(mode: i32, mods: &serde_json::Value) -> bool {
    mods.as_array().map_or(true, |mods| {
        mods.iter().all(|m| {
            let acronym = m["acronym"].as_str().unwrap_or_default();
            let has_settings = m["settings"].as_object().map_or(false, |s| !s.is_empty());

            if LAZER_ANY_SETTINGS.contains(&acronym) {
                return true;
            }

            let ranked = match acronym {
                "MR" => mode == 3,
                "HR" => mode != 3,
                _ => LAZER_DEFAULT_SETTINGS.contains(&acronym),
            };

            ranked && !has_settings
        })
    })
}

/// Recalculates the pp of passed osu!lazer scores on ranked maps, whatever their mods, the way the lazer server calculates it on submission:
/// from the stored mods and statistics, with lazer scoring. Without this, a pp change would leave them behind.
async fn recalculate_mode_lazer_scores(mode: i32, ctx: Arc<Context>) -> anyhow::Result<()> {
    let scores: Result<Vec<LazerScoreRow>, sqlx::Error> = sqlx::query_as(
        "SELECT l.id, l.beatmap_id, l.accuracy, l.max_combo, CAST(l.mods AS CHAR) AS mods,
            CAST(l.statistics AS CHAR) AS statistics, l.pp, l.ranked_mods
        FROM lazer_scores l
        INNER JOIN beatmaps b ON b.beatmap_md5 = l.beatmap_md5
        WHERE l.ruleset_id = ? AND l.passed = 1 AND b.ranked IN (2, 3)",
    )
    .bind(mode)
    .fetch_all(&ctx.database)
    .await;

    let scores = match scores {
        Ok(scores) => scores,
        // The lazer tables haven't been migrated, so there's nothing to recalculate.
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("42S02") => {
            return Ok(())
        }
        Err(error) => return Err(error.into()),
    };

    for score in scores {
        let beatmap_path =
            Path::new(&ctx.config.beatmaps_path).join(format!("{}.osu", score.beatmap_id));

        if !beatmap_path.exists() {
            log::info!("Beatmap {} doesn't exist, fetching from bancho", score.beatmap_id);

            let response = reqwest::get(&format!("https://old.ppy.sh/osu/{}", score.beatmap_id))
                .await?
                .error_for_status();

            let Ok(response) = response else {
                log::warn!("Failed to get .osu for beatmap {}", score.beatmap_id);
                continue;
            };

            let mut file = File::create(&beatmap_path).await?;
            let mut content = Cursor::new(response.bytes().await?);
            tokio::io::copy(&mut content, &mut file).await?;
        }

        let lazer_mods: serde_json::Value = serde_json::from_str(&score.mods)?;
        let ranked_mods = lazer_mods_ranked(mode, &lazer_mods);
        let statistics: serde_json::Value = serde_json::from_str(&score.statistics)?;
        let mut request = LazerCalculateRequest {
            beatmap_id: score.beatmap_id,
            mode,
            mods: 0,
            max_combo: score.max_combo,
            accuracy: (score.accuracy * 100.0) as f32,
            miss_count: statistics["miss"].as_i64().unwrap_or(0) as i32,
            passed_objects: None,
            playback_rate: None,
            lazer: true,
            lazer_mods: Some(lazer_mods),
        };
        request.mods = request.parsed_lazer_mods().map_or(0, |mods| mods.bits() as i32);

        let response = calculate(beatmap_path, &request).await;

        if response.pp != score.pp || ranked_mods != score.ranked_mods {
            sqlx::query("UPDATE lazer_scores SET pp = ?, ranked_mods = ? WHERE id = ?")
                .bind(response.pp)
                .bind(ranked_mods)
                .bind(score.id)
                .execute(&ctx.database)
                .await?;

            log::info!(
                "Recalculated lazer score ID {} (mode: {}) | {} -> {}",
                score.id,
                mode,
                score.pp,
                response.pp,
            );
        }
    }

    Ok(())
}

#[derive(sqlx::FromRow)]
struct LazerBestPlay {
    pp: f32,
    accuracy: f64,
}

/// Variant 0 is vanilla, 1 relax and 2 autopilot, matching the stable boards that exist for each mode.
fn lazer_variants(mode: i32) -> &'static [i32] {
    match mode {
        0 => &[0, 1, 2],
        1 | 2 => &[0, 1],
        _ => &[0],
    }
}

fn lazer_variant_names(variant: i32) -> (&'static str, &'static str) {
    match variant {
        0 => ("lazer_stats", "ripple:leaderboard_lazer"),
        1 => ("lazer_rx_stats", "ripple:leaderboard_lazer_relax"),
        2 => ("lazer_ap_stats", "ripple:leaderboard_lazer_ap"),
        _ => unreachable!(),
    }
}

/// Rebuilds a player's pp and accuracy for a mode and lazer variant from their best plays of that variant on each ranked beatmap.
async fn recalculate_lazer_user(
    user_id: i32,
    mode: i32,
    variant: i32,
    ctx: Arc<Context>,
) -> anyhow::Result<()> {
    let plays: Vec<LazerBestPlay> = sqlx::query_as(
        "SELECT best.pp, best.accuracy FROM (
            SELECT l.pp, l.accuracy, ROW_NUMBER() OVER (PARTITION BY l.beatmap_md5 ORDER BY l.pp DESC) AS rn
            FROM lazer_scores l
            INNER JOIN beatmaps b USING(beatmap_md5)
            WHERE l.user_id = ? AND l.ruleset_id = ? AND l.variant = ? AND l.ranked_mods = 1 AND l.passed = 1
                AND l.pp > 0 AND b.ranked IN (2, 3)
        ) best
        WHERE best.rn = 1
        ORDER BY best.pp DESC
        LIMIT 1000",
    )
    .bind(user_id)
    .bind(mode)
    .bind(variant)
    .fetch_all(&ctx.database)
    .await?;

    let mut weighted_pp = 0.0;
    let mut weighted_accuracy = 0.0;

    for (idx, play) in plays.iter().take(100).enumerate() {
        let weight = 0.95_f32.powi(idx as i32);
        weighted_pp += play.pp * weight;
        weighted_accuracy += play.accuracy as f32 * 100.0 * weight;
    }

    let top_count = plays.len().min(100) as i32;
    let (new_pp, accuracy) = if plays.is_empty() {
        (0, 0.0)
    } else {
        let bonus_pp = 416.6667 * (1.0 - 0.995_f32.powi(plays.len() as i32));
        let accuracy = weighted_accuracy / (20.0 * (1.0 - 0.95_f32.powi(top_count)));

        ((weighted_pp + bonus_pp).round() as i32, accuracy)
    };

    let stats_prefix = match mode {
        0 => "std",
        1 => "taiko",
        2 => "ctb",
        3 => "mania",
        _ => unreachable!(),
    };

    let (stats_table, leaderboard_key) = lazer_variant_names(variant);

    sqlx::query(&format!(
        "INSERT INTO {table} (id, username, pp_{prefix}, avg_accuracy_{prefix})
        SELECT id, username, ?, ? FROM users WHERE id = ?
        ON DUPLICATE KEY UPDATE pp_{prefix} = ?, avg_accuracy_{prefix} = ?",
        table = stats_table,
        prefix = stats_prefix
    ))
    .bind(new_pp)
    .bind(accuracy)
    .bind(user_id)
    .bind(new_pp)
    .bind(accuracy)
    .execute(&ctx.database)
    .await?;

    let (country, user_privileges): (String, i32) =
        sqlx::query_as("SELECT country, privileges FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&ctx.database)
            .await?;

    let mut redis_connection = ctx.redis.get_async_connection().await?;

    if user_privileges & 1 > 0 {
        let leaderboard = format!("{}:{}", leaderboard_key, stats_prefix);
        let country_leaderboard = format!("{}:{}", leaderboard, country.to_lowercase());

        if new_pp > 0 {
            let _: () = redis_connection
                .zadd(leaderboard, user_id.to_string(), new_pp)
                .await?;

            let _: () = redis_connection
                .zadd(country_leaderboard, user_id.to_string(), new_pp)
                .await?;
        } else {
            let _: () = redis_connection
                .zrem(leaderboard, user_id.to_string())
                .await?;

            let _: () = redis_connection
                .zrem(country_leaderboard, user_id.to_string())
                .await?;
        }
    }

    let _: () = redis_connection
        .publish("peppy:update_cached_stats", user_id)
        .await?;

    log::info!(
        "Recalculated lazer user {} in mode {} (variant: {}) | pp: {}",
        user_id,
        mode,
        variant,
        new_pp
    );

    Ok(())
}

/// Covers players with lazer scores in the mode, and those whose stored pp may need zeroing.
async fn recalculate_mode_lazer_users(mode: i32, ctx: Arc<Context>) -> anyhow::Result<()> {
    for &variant in lazer_variants(mode) {
        recalculate_mode_lazer_variant_users(mode, variant, ctx.clone()).await?;
    }

    Ok(())
}

async fn recalculate_mode_lazer_variant_users(
    mode: i32,
    variant: i32,
    ctx: Arc<Context>,
) -> anyhow::Result<()> {
    let stats_prefix = match mode {
        0 => "std",
        1 => "taiko",
        2 => "ctb",
        3 => "mania",
        _ => unreachable!(),
    };

    let user_ids: Result<Vec<(i32,)>, sqlx::Error> = sqlx::query_as(&format!(
        "SELECT user_id FROM lazer_scores WHERE ruleset_id = ? AND variant = ? AND ranked_mods = 1 AND passed = 1 AND pp > 0
        UNION
        SELECT id FROM {} WHERE pp_{} > 0",
        lazer_variant_names(variant).0,
        stats_prefix
    ))
    .bind(mode)
    .bind(variant)
    .fetch_all(&ctx.database)
    .await;

    let user_ids = match user_ids {
        Ok(user_ids) => user_ids,
        // The lazer tables haven't been migrated, so there are no lazer players.
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("42S02") => {
            return Ok(())
        }
        Err(error) => return Err(error.into()),
    };

    for user_id_chunk in user_ids.chunks(100).map(|c| c.to_vec()) {
        let mut futures = Vec::with_capacity(user_id_chunk.len());

        for (user_id,) in user_id_chunk {
            let future = tokio::spawn(recalculate_lazer_user(user_id, mode, variant, ctx.clone()));
            futures.push(future);
        }

        futures::future::try_join_all(futures).await?;
    }

    Ok(())
}

fn calculate_new_pp(scores: &Vec<RippleScore>, score_count: i32) -> i32 {
    let mut total_pp = 0.0;

    for (idx, score) in scores.iter().enumerate() {
        total_pp += score.pp * 0.95_f32.powi(idx as i32);
    }

    // bonus pp
    total_pp += 416.6667 * (1.0 - 0.995_f32.powi(score_count));

    total_pp.round() as i32
}

async fn recalculate_status(
    user_id: i32,
    mode: i32,
    rx: i32,
    beatmap_md5: String,
    ctx: Arc<Context>,
) -> anyhow::Result<()> {
    let scores_table = match rx {
        0 => "scores",
        1 => "scores_relax",
        2 => "scores_ap",
        _ => unreachable!(),
    };

    let scores: Vec<(i64, f32)> = sqlx::query_as(
        &format!(
            "SELECT id, pp FROM {} WHERE userid = ? AND play_mode = ? AND beatmap_md5 = ? AND completed IN (2, 3) ORDER BY pp DESC",
            scores_table
        )
    )
    .bind(user_id)
    .bind(mode)
    .bind(beatmap_md5)
    .fetch_all(&ctx.database)
    .await?;

    let best_id = scores[0].0;
    let non_bests = scores[1..].to_vec();

    sqlx::query(&format!(
        "UPDATE {} SET completed = 3 WHERE id = ?",
        scores_table
    ))
    .bind(best_id)
    .execute(&ctx.database)
    .await?;

    for non_best in non_bests {
        sqlx::query(&format!(
            "UPDATE {} SET completed = 2 WHERE id = ?",
            scores_table
        ))
        .bind(non_best.0)
        .execute(&ctx.database)
        .await?;
    }

    Ok(())
}

async fn recalculate_statuses(
    user_id: i32,
    mode: i32,
    rx: i32,
    ctx: Arc<Context>,
) -> anyhow::Result<()> {
    let scores_table = match rx {
        0 => "scores",
        1 => "scores_relax",
        2 => "scores_ap",
        _ => unreachable!(),
    };

    let beatmap_md5s: Vec<(String,)> = sqlx::query_as(
        &format!(
            "SELECT DISTINCT (beatmap_md5) FROM {} WHERE userid = ? AND completed IN (2, 3) AND play_mode = ?",
            scores_table
        )
    )
        .bind(user_id)
        .bind(mode)
        .fetch_all(&ctx.database)
        .await?;

    for beatmap_chunk in beatmap_md5s.chunks(100).map(|c| c.to_vec()) {
        let mut futures = Vec::with_capacity(beatmap_chunk.len());

        for (beatmap_md5,) in beatmap_chunk {
            let future = tokio::spawn(recalculate_status(
                user_id,
                mode,
                rx,
                beatmap_md5,
                ctx.clone(),
            ));

            futures.push(future);
        }

        futures::future::try_join_all(futures).await?;
    }

    Ok(())
}

async fn recalculate_user(
    user_id: i32,
    mode: i32,
    rx: i32,
    ctx: Arc<Context>,
) -> anyhow::Result<()> {
    recalculate_statuses(user_id, mode, rx, ctx.clone()).await?;

    let scores_table = match rx {
        0 => "scores",
        1 => "scores_relax",
        2 => "scores_ap",
        _ => unreachable!(),
    };

    let scores: Vec<RippleScore> = sqlx::query_as(
        &format!(
            "SELECT s.id, s.beatmap_md5, s.userid, s.score, s.max_combo, s.full_combo, s.mods, (s.playback_rate + 0e0) AS playback_rate, s.300_count, 
            s.100_count, s.50_count, s.katus_count, s.gekis_count, s.misses_count, s.time, s.play_mode, s.completed, 
            s.accuracy, s.pp, b.beatmap_id, b.beatmapset_id 
            FROM {} s 
            INNER JOIN 
                beatmaps b 
                USING(beatmap_md5) 
            WHERE 
                userid = ? 
                AND completed = 3 
                AND play_mode = ? 
                AND ranked IN (3, 2) 
            ORDER BY pp DESC 
            LIMIT 100",
            scores_table
        )
    )
    .bind(user_id)
    .bind(mode)
    .fetch_all(&ctx.database)
    .await?;

    let score_count: i32 = sqlx::query_scalar(
        &format!(
            "SELECT COUNT(s.id) FROM {} s INNER JOIN beatmaps USING(beatmap_md5) WHERE userid = ? AND completed = 3 AND play_mode = ? AND ranked IN (3, 2) LIMIT 1000",
            scores_table
        )
    )
        .bind(user_id)
        .bind(mode)
        .fetch_one(&ctx.database)
        .await?;

    let new_pp = calculate_new_pp(&scores, score_count);

    let stats_table = match rx {
        0 => "users_stats",
        1 => "rx_stats",
        2 => "ap_stats",
        _ => unreachable!(),
    };

    let stats_prefix = match mode {
        0 => "std",
        1 => "taiko",
        2 => "ctb",
        3 => "mania",
        _ => unreachable!(),
    };

    sqlx::query(&format!(
        "UPDATE {} SET pp_{} = ? WHERE id = ?",
        stats_table, stats_prefix
    ))
    .bind(new_pp)
    .bind(user_id)
    .execute(&ctx.database)
    .await?;

    let (country, user_privileges): (String, i32) = sqlx::query_as(
        "SELECT users.country, privileges FROM users INNER JOIN users_stats USING(id) WHERE id = ?",
    )
    .bind(user_id)
    .fetch_one(&ctx.database)
    .await?;

    let mut redis_connection = ctx.redis.get_async_connection().await?;

    // unrestricted, and set a score in the past 2 months
    if user_privileges & 1 > 0 {
        let redis_leaderboard = match rx {
            0 => "leaderboard".to_string(),
            1 => "leaderboard_relax".to_string(),
            2 => "leaderboard_ap".to_string(),
            _ => unreachable!(),
        };

        redis_connection
            .zadd(
                format!("ripple:{}:{}", redis_leaderboard, stats_prefix),
                user_id.to_string(),
                new_pp,
            )
            .await?;

        redis_connection
            .zadd(
                format!(
                    "ripple:{}:{}:{}",
                    redis_leaderboard,
                    stats_prefix,
                    country.to_lowercase()
                ),
                user_id.to_string(),
                new_pp,
            )
            .await?;
    }

    redis_connection
        .publish("peppy:update_cached_stats", user_id)
        .await?;

    log::info!(
        "Recalculated user {} in mode {} (rx: {}) | pp: {}",
        user_id,
        mode,
        rx,
        new_pp
    );

    Ok(())
}

async fn recalculate_mode_users(mode: i32, rx: i32, ctx: Arc<Context>) -> anyhow::Result<()> {
    let user_ids: Vec<(i32,)> = sqlx::query_as(&format!("SELECT id FROM users"))
        .fetch_all(&ctx.database)
        .await?;

    for user_id_chunk in user_ids.chunks(100).map(|c| c.to_vec()) {
        let mut futures = Vec::with_capacity(user_id_chunk.len());

        for (user_id,) in user_id_chunk {
            let future = tokio::spawn(recalculate_user(user_id, mode, rx, ctx.clone()));
            futures.push(future);
        }

        futures::future::try_join_all(futures).await?;
    }

    Ok(())
}

/// Recalculates only osu!lazer: every passed score on a ranked map, then each player's totals and leaderboards.
pub async fn serve_lazer(context: Context) -> anyhow::Result<()> {
    print!("Enter the modes (comma delimited) to deploy: ");
    std::io::stdout().flush().unwrap();

    let mut modes_str = String::new();
    std::io::stdin().read_line(&mut modes_str)?;
    let modes = modes_str
        .trim()
        .split(',')
        .map(|s| s.parse::<i32>().unwrap())
        .collect::<Vec<_>>();

    let context = Arc::new(context);

    for mode in modes {
        recalculate_mode_lazer_scores(mode, context.clone()).await?;
        recalculate_mode_lazer_users(mode, context.clone()).await?;
    }

    Ok(())
}

struct RecalculateContext {
    pub beatmaps: HashMap<i32, Beatmap>,
}

pub async fn serve(context: Context) -> anyhow::Result<()> {
    print!("Enter the modes (comma delimited) to deploy: ");
    std::io::stdout().flush().unwrap();

    let mut modes_str = String::new();
    std::io::stdin().read_line(&mut modes_str)?;
    let modes = modes_str
        .trim()
        .split(',')
        .map(|s| s.parse::<i32>().unwrap())
        .collect::<Vec<_>>();

    print!("\n");
    std::io::stdout().flush().unwrap();

    print!("Enter the relax bits (comma delimited) to deploy: ");
    std::io::stdout().flush().unwrap();

    let mut relax_str = String::new();
    std::io::stdin().read_line(&mut relax_str)?;
    let relax_bits = relax_str
        .trim()
        .split(',')
        .map(|s| s.parse::<i32>().unwrap())
        .collect::<Vec<_>>();

    print!("\n");
    std::io::stdout().flush().unwrap();

    let recalculate_context = Arc::new(Mutex::new(RecalculateContext {
        beatmaps: HashMap::new(),
    }));

    let context_arc = Arc::new(context);

    for mode in &modes {
        let mode = mode.clone();

        let rx = vec![0, 1, 2].contains(&mode);
        let ap = mode == 0;

        if rx || ap {
            for rx in &relax_bits {
                recalculate_mode_scores(
                    mode,
                    rx.clone(),
                    context_arc.clone(),
                    recalculate_context.clone(),
                )
                .await?;
            }
        } else {
            recalculate_mode_scores(mode, 0, context_arc.clone(), recalculate_context.clone())
                .await?;
        }
    }

    for mode in &modes {
        let mode = mode.clone();

        let rx = vec![0, 1, 2].contains(&mode);
        let ap = mode == 0;

        if rx || ap {
            for rx in &relax_bits {
                recalculate_mode_users(mode, rx.clone(), context_arc.clone()).await?;
            }
        } else {
            recalculate_mode_users(mode, 0, context_arc.clone()).await?;
        }
    }

    Ok(())
}

pub async fn recalc_single(context: Context) -> anyhow::Result<()> {
    print!("Enter the user ID of the user to recalculate: ");
    std::io::stdout().flush().unwrap();

    let mut user_id_string = String::new();
    std::io::stdin().read_line(&mut user_id_string)?;

    print!("\n");
    std::io::stdout().flush().unwrap();

    let user_id: i32 = user_id_string.trim().parse()?;

    let ctx = Arc::new(context);

    for rx in 0..3 {
        for mode in 0..4 {
            recalculate_user(user_id, mode, rx, ctx.clone()).await?;
        }
    }

    Ok(())
}

use std::sync::Arc;

use axum::{
    extract::{Extension, Path},
    routing::get,
    Json, Router,
};

use crate::{
    context::Context,
    models::{
        beatmap::Beatmap,
        score::{APIBaseReworkScore, APIReworkScore},
    },
};

pub fn router() -> Router {
    Router::new().route(
        "/api/v1/reworks/:rework_id/scores/:user_id",
        get(get_rework_scores),
    )
}

async fn get_rework_scores(
    ctx: Extension<Arc<Context>>,
    Path((rework_id, user_id)): Path<(i32, i32)>,
) -> Json<Option<Vec<APIReworkScore>>> {
    let base_scores: Vec<APIBaseReworkScore> =
        sqlx::query_as(
            "SELECT user_id, beatmap_id, beatmapset_id, rework_id, score_id, max_combo, mods, accuracy, score, num_300s, num_100s, num_50s, num_gekis, 
            num_katus, num_misses, old_pp, new_pp 
            FROM 
                rework_scores 
            WHERE 
                user_id = ? AND rework_id = ?
            ORDER BY 
                new_pp DESC
            LIMIT 100",
        )
            .bind(user_id)
            .bind(rework_id)
            .fetch_all(&ctx.database)
            .await
            .unwrap();

    let mut scores: Vec<APIReworkScore> = Vec::new();
    for base_score in &base_scores {
        let beatmap: Beatmap = sqlx::query_as(
            "SELECT beatmap_id, beatmapset_id, song_name FROM beatmaps WHERE beatmap_id = ?",
        )
        .bind(base_score.beatmap_id)
        .fetch_one(&ctx.database)
        .await
        .unwrap();

        let mut temp_scores = base_scores.clone();

        temp_scores.sort_by(|a, b| a.old_pp.partial_cmp(&b.old_pp).unwrap());
        temp_scores.reverse();
        let old_rank = (temp_scores
            .iter()
            .position(|a| a.score_id == base_score.score_id)
            .unwrap()
            + 1) as u64;

        temp_scores.sort_by(|a, b| a.new_pp.partial_cmp(&b.new_pp).unwrap());
        temp_scores.reverse();
        let new_rank = (temp_scores
            .iter()
            .position(|a| a.score_id == base_score.score_id)
            .unwrap()
            + 1) as u64;

        let score = APIReworkScore::from_base(base_score.clone(), beatmap, old_rank, new_rank);
        scores.push(score);
    }

    match scores.is_empty() {
        true => Json(None),
        false => Json(Some(scores)),
    }
}

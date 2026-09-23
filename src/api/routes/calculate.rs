use crate::config::Config;
use akatsuki_pp_rs::{any::PerformanceAttributes, model::mode::GameMode, Beatmap};
use axum::{
    extract::Extension,
    routing::{get, post},
    Json, Router,
};
use rosu_mods::{
    serde::GameModsSeed, GameModIntermode, GameMode as LazerGameMode, GameMods as LazerMods,
};
use serde::de::DeserializeSeed;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;

pub fn router() -> Router {
    Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/calculate", post(calculate_play))
}

#[derive(serde::Serialize)]
struct ServiceStatus {
    status: i32,
    online: bool,
}

// TODO: move this somewhere else.
async fn status() -> Json<ServiceStatus> {
    let res = ServiceStatus {
        status: 200,
        online: true,
    };

    Json(res)
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CalculateRequest {
    pub beatmap_id: i32,
    pub mode: i32,
    pub mods: i32,
    pub max_combo: i32,
    pub accuracy: f32,
    pub miss_count: i32,
    pub passed_objects: Option<i32>,
    pub playback_rate: Option<f32>,
    /// True when the score was submitted from osu!lazer rather than stable. Selects
    /// lazer scoring-v2-aware difficulty/performance calculation (see rosu-pp's
    /// `.lazer()`) on the modern (non-2019) calculation path. Defaults to false so
    /// existing stable-only callers are unaffected.
    #[serde(default)]
    pub lazer: bool,
    /// Lazer's full mod list (acronym + settings), e.g. `[{"acronym":"DT","settings":
    /// {"speed_change":1.2}}]` — same wire shape as the client sends. When present,
    /// the modern calculation path (calculate_rosu_pp) uses this instead of `mods`
    /// so mod settings (custom DT/HT rate, etc.) and lazer-exclusive mods with no
    /// legacy bit are accounted for. The 2019 path (calculate_relax_pp) can't accept
    /// rich mods at all — it only pulls the clock rate out of this, everything else
    /// about mod semantics there still comes from the legacy `mods` bitfield.
    ///
    /// Kept as raw JSON rather than `LazerMods` directly: rosu_mods mod
    /// deserialization is mode-dependent (an acronym maps to a different concrete
    /// mod struct per ruleset) and needs `mode` as seed context, which plain derived
    /// `Deserialize` can't provide from a sibling field. See `parsed_lazer_mods()`.
    #[serde(default)]
    pub lazer_mods: Option<serde_json::Value>,
}

impl CalculateRequest {
    fn parsed_lazer_mods(&self) -> Option<LazerMods> {
        let value = self.lazer_mods.clone()?;
        let mode = match self.mode {
            0 => LazerGameMode::Osu,
            1 => LazerGameMode::Taiko,
            2 => LazerGameMode::Catch,
            3 => LazerGameMode::Mania,
            _ => return None,
        };

        GameModsSeed::Mode {
            mode,
            deny_unknown_fields: false,
        }
        .deserialize(value)
        .ok()
    }

    /// Classic (CL) has no legacy bitfield bit — it's lazer-only — so it can only be
    /// read out of `lazer_mods`. Stable-only callers never send that field, so this
    /// is naturally always false for them.
    fn classic(&self) -> bool {
        self.parsed_lazer_mods()
            .is_some_and(|m| m.contains_intermode(GameModIntermode::Classic))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CalculateResponse {
    pub stars: f32,
    pub pp: f32,
    pub ar: f32,
    pub od: f32,
    pub max_combo: i32,
}

fn round(x: f32, decimals: u32) -> f32 {
    let y = 10i32.pow(decimals) as f32;
    (x * y).round() / y
}

async fn calculate_relax_pp(
    beatmap_path: PathBuf,
    request: &CalculateRequest,
) -> CalculateResponse {
    let beatmap = match Beatmap::from_path(beatmap_path) {
        Ok(beatmap) => beatmap,
        Err(_) => {
            return CalculateResponse {
                stars: 0.0,
                pp: 0.0,
                ar: 0.0,
                od: 0.0,
                max_combo: 0,
            }
        }
    };

    let mut builder = akatsuki_pp_rs::osu_2019::OsuPP::from_map(&beatmap)
        .mods(request.mods as u32)
        .combo(request.max_combo as u32)
        .misses(request.miss_count as u32)
        .accuracy(request.accuracy);

    if let Some(passed_objects) = request.passed_objects {
        builder = builder.passed_objects(passed_objects as u32);
    }

    // osu_2019::OsuPP only understands the legacy `mods` bitfield (set above), so
    // rich lazer mod semantics (AS, DA, etc.) are lost here regardless — we only
    // pull the clock rate out of lazer_mods, since custom DT/HT rate is otherwise
    // silently dropped by the bitfield. Falls back to the explicit playback_rate
    // field for callers that don't send lazer_mods.
    let clock_rate = request
        .parsed_lazer_mods()
        .and_then(|m| m.clock_rate())
        .or(request.playback_rate.map(|r| r as f64));

    if let Some(clock_rate) = clock_rate {
        builder = builder.clock_rate(clock_rate);
    }

    let result = builder.calculate();

    let mut pp = round(result.pp as f32, 2);
    if pp.is_infinite() || pp.is_nan() {
        pp = 0.0;
    }

    let mut stars = round(result.difficulty.stars as f32, 2);
    if stars.is_infinite() || stars.is_nan() {
        stars = 0.0;
    }

    CalculateResponse {
        stars,
        pp,
        ar: result.difficulty.ar as f32,
        od: result.difficulty.od as f32,
        max_combo: result.difficulty.max_combo as i32,
    }
}

async fn calculate_rosu_pp(beatmap_path: PathBuf, request: &CalculateRequest) -> CalculateResponse {
    let beatmap = match Beatmap::from_path(beatmap_path) {
        Ok(beatmap) => beatmap,
        Err(_) => {
            return CalculateResponse {
                stars: 0.0,
                pp: 0.0,
                ar: 0.0,
                od: 0.0,
                max_combo: 0,
            }
        }
    };

    let mut builder = beatmap
        .performance()
        .try_mode(match request.mode {
            0 => GameMode::Osu,
            1 => GameMode::Taiko,
            2 => GameMode::Catch,
            3 => GameMode::Mania,
            _ => unreachable!(),
        })
        .unwrap()
        .lazer(request.lazer)
        .combo(request.max_combo as u32)
        .accuracy(request.accuracy as f64)
        .misses(request.miss_count as u32);

    // Prefer the full lazer mod list when present (carries settings like custom
    // DT/HT rate and lazer-exclusive mods with no legacy bit) over the legacy
    // bitfield. rosu-pp derives clock rate from the mods object itself here, so
    // (unlike the 2019 path) no separate playback_rate handling is needed once
    // real mods are passed through — an explicit rate field would just be a second,
    // possibly-conflicting source of truth for something the mods already encode.
    builder = match request.parsed_lazer_mods() {
        Some(lazer_mods) => builder.mods(lazer_mods),
        None => builder.mods(request.mods as u32),
    };

    if let Some(passed_objects) = request.passed_objects {
        builder = builder.passed_objects(passed_objects as u32);
    }

    let result = builder.calculate();

    let mut pp = round(result.pp() as f32, 2);
    if pp.is_infinite() || pp.is_nan() {
        pp = 0.0;
    }

    let mut stars = round(result.stars() as f32, 2);
    if stars.is_infinite() || stars.is_nan() {
        stars = 0.0;
    }

    match result {
        PerformanceAttributes::Osu(result) => CalculateResponse {
            stars,
            pp,
            ar: result.difficulty.ar as f32,
            od: result.difficulty.od() as f32, // Why is it a function now
            max_combo: result.difficulty.max_combo as i32,
        },
        PerformanceAttributes::Taiko(result) => CalculateResponse {
            stars,
            pp,
            ar: 0.0,
            od: 0.0,
            max_combo: result.difficulty.max_combo as i32,
        },
        PerformanceAttributes::Catch(result) => CalculateResponse {
            stars,
            pp,
            ar: 0.0,
            od: 0.0,
            max_combo: result.difficulty.max_combo() as i32,
        },
        PerformanceAttributes::Mania(result) => CalculateResponse {
            stars,
            pp,
            ar: 0.0,
            od: 0.0,
            max_combo: result.difficulty.max_combo as i32,
        },
    }
}

const RX: i32 = 1 << 7;

async fn download_beatmap(beatmap_path: PathBuf, request: &CalculateRequest) -> anyhow::Result<()> {
    let response = reqwest::get(&format!("https://old.ppy.sh/osu/{}", request.beatmap_id))
        .await?
        .error_for_status()?;

    let mut file = File::create(&beatmap_path).await?;
    let mut content = Cursor::new(response.bytes().await?);
    tokio::io::copy(&mut content, &mut file).await?;

    Ok(())
}

async fn calculate_play(
    Extension(config): Extension<Arc<Config>>,
    Json(requests): Json<Vec<CalculateRequest>>,
) -> Json<Vec<CalculateResponse>> {
    let mut results = Vec::new();

    for request in requests {
        let beatmap_path =
            Path::new(&config.beatmaps_path).join(format!("{}.osu", request.beatmap_id));

        if !beatmap_path.exists() {
            match download_beatmap(beatmap_path.clone(), &request).await {
                Ok(_) => {}
                Err(_) => {
                    results.push(CalculateResponse {
                        stars: 0.0,
                        pp: 0.0,
                        ar: 0.0,
                        od: 0.0,
                        max_combo: 0,
                    });

                    continue;
                }
            }
        }

        // osu_2019::OsuPP (calculate_relax_pp) is std-only, hence `mode == 0` gating
        // both branches below. Classic-modded scores use the same 2019 algorithm as
        // Relax since CL deliberately reverts scoring/difficulty to stable-era rules.
        let use_2019_pp = request.mode == 0 && (request.mods & RX > 0 || request.classic());

        let result = if use_2019_pp {
            calculate_relax_pp(beatmap_path, &request).await
        } else {
            calculate_rosu_pp(beatmap_path, &request).await
        };

        results.push(result);
    }

    Json(results)
}

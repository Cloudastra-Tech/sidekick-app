//! Test call report: pools every live call (not demos or solo tests) and checks the Phase 2
//! success criteria across them, so the go/no-go uses measured numbers from real calls. Only
//! calls of 10+ minutes count toward the targets; shorter ones are listed but left out.
//!
//! Sources: each call's structured record (`calls/call-<ms>.json`), its metrics log (per
//! model call latency), and `metrics/hint_ratings.jsonl` (latest rating per hint).

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tauri::{AppHandle, Manager};

/// Phase 2 targets (CLAUDE.md, roadmap.md).
const TARGET_LATENCY_P90_MS: f64 = 3000.0;
const TARGET_CPU_AVG_PERCENT: f64 = 15.0;
/// Top of the roadmap's modeled $0.30–1.00 per call-hour.
const TARGET_COST_PER_HOUR_USD: f64 = 1.00;
/// Calls shorter than this don't show steady-state cost or cache behavior.
const MIN_REAL_CALL_MINUTES: f64 = 10.0;
/// Calls the roadmap asks for before the Decision Gate.
const CALLS_NEEDED: usize = 3;

#[derive(Serialize, Default, Debug)]
pub struct CallRow {
    id: String,
    title: String,
    minutes: f64,
    hints_shown: u64,
    rated_up: u64,
    rated_down: u64,
    latency_p90_ms: Option<f64>,
    cpu_avg_percent: Option<f64>,
    cost_per_hour_usd: Option<f64>,
    /// Seconds of audio sent to STT per second of call, across both streams (0–1).
    stt_sent_ratio: Option<f64>,
    stream_errors: usize,
    short: bool,
}

#[derive(Serialize, Debug)]
pub struct Check {
    label: &'static str,
    target: String,
    value: Option<String>,
    /// `pass`, `fail`, or `no_data`.
    status: &'static str,
}

#[derive(Serialize, Debug)]
pub struct Report {
    calls: Vec<CallRow>,
    /// Live calls of at least 10 minutes, and how many the Decision Gate asks for.
    real_calls: usize,
    calls_needed: usize,
    total_minutes: f64,
    checks: Vec<Check>,
    /// Other numbers worth tracking that have no target.
    rated_useful_percent: Option<f64>,
    hints_rated: u64,
    hints_shown: u64,
    cache_hit_percent: Option<f64>,
    stt_sent_percent: Option<f64>,
    summary_cost_usd: f64,
}

/// One live call's inputs.
pub struct CallInput {
    pub id: String,
    pub record: Value,
    /// `speech_to_hint` of each model call, and whether it was a keyword/turn-end trigger.
    pub latencies: Vec<(f64, bool)>,
}

fn percentile(values: &mut [f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let index = ((values.len() as f64 - 1.0) * p).round() as usize;
    Some(values[index])
}

fn check(
    label: &'static str,
    target: String,
    value: Option<(f64, String)>,
    pass: impl Fn(f64) -> bool,
) -> Check {
    match value {
        Some((number, shown)) => Check {
            label,
            target,
            value: Some(shown),
            status: if pass(number) { "pass" } else { "fail" },
        },
        None => Check {
            label,
            target,
            value: None,
            status: "no_data",
        },
    }
}

/// Builds the report from live calls and the latest rating of each hint.
pub fn build(calls: Vec<CallInput>, ratings: &HashMap<String, String>) -> Report {
    let f = |v: &Value| v.as_f64();
    let mut rows = Vec::new();
    let mut responsive_latencies = Vec::new();
    let (mut total_minutes, mut total_cost, mut cost_minutes) = (0.0, 0.0, 0.0);
    let (mut cpu_weighted, mut cpu_minutes) = (0.0, 0.0);
    let (mut input_tokens, mut cache_read) = (0.0, 0.0);
    let (mut stt_minutes, mut stt_call_minutes) = (0.0, 0.0);
    let (mut up_total, mut down_total, mut shown_total) = (0, 0, 0);
    let mut summary_cost = 0.0;

    for call in calls {
        let stats = &call.record["stats"];
        let minutes = f(&stats["duration_minutes"]).unwrap_or(0.0);
        let stem = stats["metrics_file"]
            .as_str()
            .and_then(|p| Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().into_owned());
        let (mut up, mut down) = (0, 0);
        if let Some(stem) = &stem {
            let prefix = format!("{stem}#");
            let call_ratings = ratings
                .iter()
                .filter(|(hint, _)| hint.starts_with(&prefix))
                .map(|(_, rating)| rating);
            for rating in call_ratings {
                match rating.as_str() {
                    "up" => up += 1,
                    "down" => down += 1,
                    _ => {}
                }
            }
        }
        let mut call_latencies: Vec<f64> = call
            .latencies
            .iter()
            .filter(|(_, responsive)| *responsive)
            .map(|(ms, _)| *ms)
            .collect();
        let short = minutes < MIN_REAL_CALL_MINUTES;
        let hints_shown = stats["hints_shown"].as_u64().unwrap_or(0);
        let cost = f(&stats["cost_usd"]["total"]);
        let cpu = f(&stats["cpu_percent"]["avg"]);
        let sent = f(&stats["stt_minutes"]["me"]).unwrap_or(0.0)
            + f(&stats["stt_minutes"]["them"]).unwrap_or(0.0);
        let mut row = CallRow {
            title: call.record["title"].as_str().unwrap_or("Call").to_string(),
            minutes,
            hints_shown,
            rated_up: up,
            rated_down: down,
            latency_p90_ms: percentile(&mut call_latencies, 0.9),
            cpu_avg_percent: cpu,
            cost_per_hour_usd: cost.filter(|_| minutes > 0.0).map(|c| c / (minutes / 60.0)),
            stt_sent_ratio: (minutes > 0.0).then(|| sent / (minutes * 2.0)),
            stream_errors: stats["stream_errors"].as_array().map_or(0, Vec::len),
            short,
            id: String::new(),
        };
        row.id = call.id;
        rows.push(row);
        if short {
            continue;
        }
        responsive_latencies.extend(call_latencies.iter().copied());

        if let Some(cost) = cost {
            total_cost += cost;
            cost_minutes += minutes;
        }
        if let Some(cpu) = cpu {
            cpu_weighted += cpu * minutes;
            cpu_minutes += minutes;
        }
        input_tokens += f(&stats["tokens"]["input"]).unwrap_or(0.0);
        cache_read += f(&stats["tokens"]["cache_read"]).unwrap_or(0.0);
        if minutes > 0.0 {
            stt_minutes += sent;
            stt_call_minutes += minutes * 2.0;
        }
        if let Some(cost) = f(&call.record["call"]["cost_usd"]) {
            summary_cost += cost;
        }
        up_total += up;
        down_total += down;
        shown_total += hints_shown;
        total_minutes += minutes;
    }

    let latency =
        percentile(&mut responsive_latencies, 0.9).map(|ms| (ms, format!("{:.1} s", ms / 1000.0)));
    let cpu = (cpu_minutes > 0.0)
        .then(|| cpu_weighted / cpu_minutes)
        .map(|c| (c, format!("{c:.1}%")));
    let cost_per_hour = (cost_minutes > 0.0)
        .then(|| (total_cost + summary_cost) / (cost_minutes / 60.0))
        .map(|c| (c, format!("${c:.2}")));
    let real_calls = rows.iter().filter(|r| !r.short).count();

    Report {
        checks: vec![
            check(
                "Hint latency p90 (keyword and turn-end)",
                format!("≤ {:.0} s", TARGET_LATENCY_P90_MS / 1000.0),
                latency,
                |ms| ms <= TARGET_LATENCY_P90_MS,
            ),
            check(
                "Cost per call-hour (hints, transcription, summary)",
                format!("≤ ${TARGET_COST_PER_HOUR_USD:.2}"),
                cost_per_hour,
                |c| c <= TARGET_COST_PER_HOUR_USD,
            ),
            check(
                "CPU during calls (average)",
                format!("< {TARGET_CPU_AVG_PERCENT:.0}%"),
                cpu,
                |c| c < TARGET_CPU_AVG_PERCENT,
            ),
            check(
                "Real test calls (10+ minutes)",
                format!("≥ {CALLS_NEEDED}"),
                Some((real_calls as f64, real_calls.to_string())),
                |n| n as usize >= CALLS_NEEDED,
            ),
        ],
        rated_useful_percent: (up_total + down_total > 0)
            .then(|| 100.0 * up_total as f64 / (up_total + down_total) as f64),
        hints_rated: up_total + down_total,
        hints_shown: shown_total,
        cache_hit_percent: (input_tokens > 0.0).then(|| 100.0 * cache_read / input_tokens),
        stt_sent_percent: (stt_call_minutes > 0.0).then(|| 100.0 * stt_minutes / stt_call_minutes),
        summary_cost_usd: summary_cost,
        real_calls,
        calls_needed: CALLS_NEEDED,
        total_minutes,
        calls: rows,
    }
}

/// `(speech_to_hint ms, keyword or turn-end)` for each model call in a metrics log.
fn read_latencies(path: &Path) -> Vec<(f64, bool)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["kind"] == "llm_call")
        .filter_map(|entry| {
            let ms = entry["latency_ms"]["speech_to_hint"].as_f64()?;
            Some((ms, entry["reason"]["kind"] != "silence"))
        })
        .collect()
}

/// Latest rating per hint ID (`up`, `down`, or cleared).
fn read_ratings(path: &Path) -> HashMap<String, String> {
    let mut ratings = HashMap::new();
    if let Ok(text) = fs::read_to_string(path) {
        for entry in text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        {
            if let Some(id) = entry["hint_id"].as_str() {
                ratings.insert(
                    id.to_string(),
                    entry["rating"].as_str().unwrap_or("").to_string(),
                );
            }
        }
    }
    ratings
}

#[tauri::command]
pub fn test_call_report(app: AppHandle) -> Result<Report, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let calls_dir = data.join("calls");
    let mut calls = Vec::new();
    if let Ok(entries) = fs::read_dir(&calls_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(record) = fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            else {
                continue;
            };
            if record["mode"] != "live" {
                continue;
            }
            let latencies = record["stats"]["metrics_file"]
                .as_str()
                .map(|p| read_latencies(Path::new(p)))
                .unwrap_or_default();
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            calls.push(CallInput {
                id,
                record,
                latencies,
            });
        }
    }
    calls.sort_by(|a, b| b.id.cmp(&a.id));
    let ratings = read_ratings(&data.join("metrics").join("hint_ratings.jsonl"));
    Ok(build(calls, &ratings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, minutes: f64, cost: f64, cpu: f64, latencies: Vec<(f64, bool)>) -> CallInput {
        CallInput {
            id: id.into(),
            record: json!({
                "title": id,
                "mode": "live",
                "call": { "status": "ready", "cost_usd": 0.02 },
                "stats": {
                    "duration_minutes": minutes,
                    "hints_shown": 4,
                    "cpu_percent": { "avg": cpu },
                    "tokens": { "input": 1000, "cache_read": 800 },
                    "stt_minutes": { "me": minutes * 0.3, "them": minutes * 0.5 },
                    "stream_errors": [],
                    "cost_usd": { "total": cost },
                    "metrics_file": format!("C:/x/metrics/session-{id}.jsonl"),
                }
            }),
            latencies,
        }
    }

    #[test]
    fn pools_calls_and_checks_targets() {
        let mut ratings = HashMap::new();
        ratings.insert("session-a#1".to_string(), "up".to_string());
        ratings.insert("session-a#2".to_string(), "down".to_string());
        ratings.insert("session-b#1".to_string(), "up".to_string());
        ratings.insert("session-b#2".to_string(), String::new());
        let calls = vec![
            call(
                "a",
                30.0,
                0.20,
                2.0,
                vec![(1200.0, true), (1800.0, true), (5000.0, false)],
            ),
            call("b", 30.0, 0.30, 4.0, vec![(2500.0, true), (4000.0, true)]),
            call("c", 5.0, 0.01, 1.0, vec![]),
        ];
        let report = build(calls, &ratings);

        assert_eq!(report.real_calls, 2);
        // Silence triggers are left out of the latency target.
        let latency = &report.checks[0];
        assert_eq!(
            (latency.status, latency.value.as_deref()),
            ("fail", Some("4.0 s"))
        );
        // Real calls only: (0.20 + 0.30 + 2 summaries at 0.02) over 60 minutes.
        let cost = &report.checks[1];
        assert_eq!(
            (cost.status, cost.value.as_deref()),
            ("pass", Some("$0.54"))
        );
        assert_eq!(report.checks[2].status, "pass");
        assert_eq!(report.checks[3].status, "fail");
        assert_eq!(
            (report.hints_rated, report.rated_useful_percent),
            (3, Some(200.0 / 3.0))
        );
        assert_eq!(report.stt_sent_percent, Some(40.0));
        assert!(report.calls[2].short);
    }

    #[test]
    fn short_calls_are_listed_but_not_counted() {
        let report = build(
            vec![call("short", 3.0, 0.01, 1.0, vec![(900.0, true)])],
            &HashMap::new(),
        );
        assert_eq!(report.calls.len(), 1);
        assert_eq!(report.checks[0].status, "no_data");
        assert_eq!(report.checks[1].status, "no_data");
        assert_eq!(report.checks[3].status, "fail");
    }
}

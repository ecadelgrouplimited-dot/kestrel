//! Persistent token-usage accounting for the Usage dashboard.
//!
//! Every model request appends one [`UsageRecord`] to the project's
//! `.kestrel/usage.jsonl`, so Kestrel can show all-time totals, a per-model
//! breakdown, and — the headline for API users — how much **prompt caching**
//! saved. It's plain JSON lines: cheap to append, easy to inspect.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One model request's token usage and cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Unix epoch seconds.
    pub ts: u64,
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

/// The usage log for a project.
pub fn usage_log_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("usage.jsonl")
}

/// Append a usage record (best-effort).
pub fn append_usage_record(root: &Path, record: &UsageRecord) {
    let path = usage_log_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(line) = serde_json::to_string(record) {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Load all usage records for a project.
pub fn load_usage_records(root: &Path) -> Vec<UsageRecord> {
    std::fs::read_to_string(usage_log_path(root))
        .map(|text| {
            text.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Aggregate totals over a set of records.
#[derive(Debug, Clone, Default)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub requests: usize,
}

impl UsageTotals {
    fn add(&mut self, r: &UsageRecord) {
        self.input += r.input;
        self.output += r.output;
        self.cache_read += r.cache_read;
        self.cache_write += r.cache_write;
        self.cost += r.cost;
        self.requests += 1;
    }
}

/// A summary of usage: overall totals, a per-model breakdown, and the estimated
/// savings from prompt caching (cache reads bill at ~10% of input).
#[derive(Debug, Clone, Default)]
pub struct UsageSummary {
    pub totals: UsageTotals,
    pub by_model: Vec<(String, UsageTotals)>,
    pub saved_cost: f64,
    pub saved_tokens: u64,
}

/// Compute the cache savings a record represents (90% off its cache reads).
pub fn record_savings(record: &UsageRecord) -> f64 {
    crate::pricing::model_price(&record.model)
        .map(|p| record.cache_read as f64 * p.input_per_million * 0.9 / 1_000_000.0)
        .unwrap_or(0.0)
}

/// Summarize a set of usage records.
pub fn summarize_usage(records: &[UsageRecord]) -> UsageSummary {
    let mut totals = UsageTotals::default();
    let mut by: std::collections::BTreeMap<String, UsageTotals> = std::collections::BTreeMap::new();
    let mut saved_cost = 0.0;
    let mut saved_tokens = 0u64;
    for r in records {
        totals.add(r);
        by.entry(r.model.clone()).or_default().add(r);
        saved_tokens += r.cache_read;
        saved_cost += record_savings(r);
    }
    UsageSummary {
        totals,
        by_model: by.into_iter().collect(),
        saved_cost,
        saved_tokens,
    }
}

/// Current Unix epoch seconds (for a record timestamp).
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The UTC midnight (epoch seconds) of the day containing `now`.
pub fn start_of_day_utc(now: u64) -> u64 {
    now - now % 86_400
}

/// Format an epoch-seconds timestamp as `YYYY-MM-DD HH:MM:SS` UTC (no deps).
pub fn format_ts(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let rem = ts % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}")
}

/// Export usage records as CSV text.
pub fn usage_csv(records: &[UsageRecord]) -> String {
    let mut csv =
        String::from("timestamp,provider,model,input,output,cache_read,cache_write,cost\n");
    for r in records {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{:.6}\n",
            format_ts(r.ts),
            r.provider,
            r.model,
            r.input,
            r.output,
            r.cache_read,
            r.cache_write,
            r.cost
        ));
    }
    csv
}

/// Total cost of records at or after `since` (epoch seconds).
pub fn cost_since(records: &[UsageRecord], since: u64) -> f64 {
    records
        .iter()
        .filter(|r| r.ts >= since)
        .map(|r| r.cost)
        .sum()
}

/// Total cost of records logged today (UTC).
pub fn cost_today(records: &[UsageRecord]) -> f64 {
    cost_since(records, start_of_day_utc(now_epoch()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(model: &str, input: u64, cache_read: u64, cost: f64) -> UsageRecord {
        UsageRecord {
            ts: 0,
            provider: "anthropic".to_string(),
            model: model.to_string(),
            input,
            output: 100,
            cache_read,
            cache_write: 0,
            cost,
        }
    }

    #[test]
    fn round_trips_and_summarizes() {
        let dir = std::env::temp_dir().join(format!("kestrel-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        append_usage_record(&dir, &rec("claude-opus-4-8", 1000, 500, 0.01));
        append_usage_record(&dir, &rec("claude-opus-4-8", 2000, 0, 0.02));

        let records = load_usage_records(&dir);
        assert_eq!(records.len(), 2);
        let summary = summarize_usage(&records);
        assert_eq!(summary.totals.requests, 2);
        assert_eq!(summary.totals.input, 3000);
        assert_eq!(summary.totals.cache_read, 500);
        assert_eq!(summary.by_model.len(), 1);
        // 500 cache-read @ 90% of $5/1M = 500 * 5 * 0.9 / 1e6 = $0.00225
        assert!((summary.saved_cost - 0.00225).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

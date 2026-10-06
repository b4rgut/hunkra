//! Fixed-size duration/virtualization counters; measurements never retain paths, source bytes or per-event logs.
use std::time::Duration;

#[derive(Default)]
pub struct Timing {
    count: u64,
    total: Duration,
    maximum: Duration,
}
impl Timing {
    /// Aggregates wall time without accumulating an unbounded list of samples.
    pub fn record(&mut self, duration: Duration) {
        self.count = self.count.saturating_add(1);
        self.total = self.total.saturating_add(duration);
        self.maximum = self.maximum.max(duration);
    }
    /// Distinguishes an unmeasured phase from a measured zero and exposes aggregate milliseconds with explicit sample count.
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"count":self.count,"mean_ms":(self.count>0).then(||self.total.as_secs_f64()*1000./self.count as f64),"max_ms":(self.count>0).then_some(self.maximum.as_secs_f64()*1000.)})
    }
}
#[derive(Default)]
pub struct Measurements {
    pub status: Timing,
    pub diff_sources: Timing,
    pub diff_prepare: Timing,
    pub diff_service: Timing,
    pub ui_prepare: Timing,
    pub file_rows_prepare: Timing,
    pub diff_rows_prepare: Timing,
    pub next_frame: Timing,
    pub watch_hints: u64,
    pub cache: hunkra_core::DiffCacheStats,
    pub file_rows: usize,
    pub diff_rows: usize,
}
impl Measurements {
    /// Reports accepted worker results and native preparation/callback wall time separately; GPU execution is explicitly unmeasured.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"status_service":self.status.json(),"diff_sources":self.diff_sources.json(),"diff_alignment_prepare":self.diff_prepare.json(),"selected_diff_service":self.diff_service.json(),"main_render_prepare_wall":self.ui_prepare.json(),"visible_file_rows_prepare_wall":self.file_rows_prepare.json(),"visible_diff_rows_prepare_wall":self.diff_rows_prepare.json(),"next_frame_callback_wall":self.next_frame.json(),"maximum_file_rows_per_batch":self.file_rows,"maximum_diff_rows_per_batch":self.diff_rows,"watch_hints":self.watch_hints,"cache":{"entries":self.cache.entries,"allocated_bytes":self.cache.allocated_bytes,"hits":self.cache.hits,"misses":self.cache.misses},"memory":process_memory(),"gpu_execution_measured":false,"scope":"accepted workers; main/visible-row CPU-code preparation wall time; next-frame callback wall latency includes scheduling, not isolated GPU execution"})
    }
}
/// Reads Linux RSS/peak RSS in KiB without confusing unsupported/missing values with zero memory usage.
pub fn process_memory() -> serde_json::Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let value = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
    };
    serde_json::json!({"rss_kib":value("VmRSS:"),"peak_rss_kib":value("VmHWM:")})
}

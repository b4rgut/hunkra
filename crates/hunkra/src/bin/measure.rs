//! Finite read-only backend measurements; callers create and mutate their own disposable fixtures.
use hunkra_core::{
    CoreError, DiffKind, DiffReader, prepare_operation, read_branches, read_diff, scan,
};
use std::{path::Path, sync::Arc, time::Instant};

/// Captures Linux process memory in KiB; other platforms explicitly report no measurement.
fn memory() -> serde_json::Value {
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
/// Measures the exact UI status bundle, including its index/HEAD agreement check, without writing Git state.
fn bundle(root: &Path) -> Result<(), CoreError> {
    let repository = scan(root)?;
    let branches = read_branches(root)?;
    let operation = prepare_operation(root)?;
    if repository.commit != branches.repository || repository.commit != operation.repository {
        return Err(CoreError::Stale);
    }
    Ok(())
}
/// Runs bounded samples and reports backend/source alignment separately from any unmeasured native GPU work.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let uncached = args.first().is_some_and(|arg| arg == "--uncached");
    if uncached {
        args.remove(0);
    }
    if args.len() < 2 {
        return Err("measure requires ROOT and one or more relative paths".into());
    }
    let root = Path::new(&args[0]);
    let mut scans = Vec::new();
    let mut bundles = Vec::new();
    let mut diffs = Vec::new();
    let mut files = 0;
    let mut reader = DiffReader::default();
    for _ in 0..3 {
        let start = Instant::now();
        files = scan(root)?.files.len();
        scans.push(start.elapsed().as_secs_f64() * 1000.);
        let start = Instant::now();
        bundle(root)?;
        bundles.push(start.elapsed().as_secs_f64() * 1000.);
        for path in &args[1..] {
            let start = Instant::now();
            let (diff, reused, source_ms, prepare_ms) = if uncached {
                (
                    Arc::new(read_diff(root, Path::new(path), DiffKind::Unstaged)?),
                    false,
                    None,
                    None,
                )
            } else {
                let read = reader.read(root, Path::new(path), DiffKind::Unstaged)?;
                (
                    read.snapshot,
                    read.reused,
                    Some(read.source_time.as_secs_f64() * 1000.),
                    Some(read.prepare_time.as_secs_f64() * 1000.),
                )
            };
            diffs.push(serde_json::json!({"path":path.to_string_lossy(),"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"source_ms":source_ms,"prepare_ms":prepare_ms,"rows":diff.rows.len(),"presentation":format!("{:?}",diff.presentation),"source_bytes":diff.left.bytes.len()+diff.right.bytes.len(),"reused":reused}));
        }
    }
    let stats = reader.stats();
    println!(
        "{}",
        serde_json::json!({"files":files,"scan_ms":scans,"bundle_ms":bundles,"diffs":diffs,"cache":{"entries":stats.entries,"allocated_bytes":stats.allocated_bytes,"hits":stats.hits,"misses":stats.misses},"uncached":uncached,"memory":memory(),"native_render_measured":false,"scope":"three warm-filesystem process-local rounds; read-only debug build"})
    );
    Ok(())
}

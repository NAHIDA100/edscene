//! Append-only NDJSON diagnostics. Opening, writing and closing per line keeps
//! an idle process free of file handles; rotation is a size check, not a daemon.
use std::io::Write;
use std::path::Path;

const MAX_BYTES: u64 = 1024 * 1024;

pub fn file(dir: &Path) -> std::path::PathBuf {
    dir.join("events.ndjson")
}

pub fn write(dir: &Path, obj: &serde_json::Value) {
    if let Err(e) = write_inner(dir, obj) {
        // Diagnostics must never take the broadcast down with them.
        eprintln!("edscene: cannot write log: {e}");
    }
}

fn write_inner(dir: &Path, obj: &serde_json::Value) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dir)?;
    let p = file(dir);
    if let Ok(md) = std::fs::metadata(&p) {
        if md.len() > MAX_BYTES {
            let old = dir.join("events.ndjson.1");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&p, &old);
        }
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p)?;
    let mut line = serde_json::to_string(obj).unwrap_or_else(|_| "{}".into());
    line.push('\n');
    f.write_all(line.as_bytes())
}

/// Three decimals, so the NDJSON does not carry f32 noise.
pub fn gain(x: f32) -> f64 {
    (f64::from(x) * 1000.0).round() / 1000.0
}

/// Monotonic milliseconds, the clock the dispatcher runs on.
pub fn mono_ms() -> u64 {
    std::time::Instant::now()
        .checked_duration_since(*ANCHOR.get_or_init(std::time::Instant::now))
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static ANCHOR: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

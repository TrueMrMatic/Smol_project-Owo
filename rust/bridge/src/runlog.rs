use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const BUILD_ID: &str = "PATCH_001_PROTOCOL_INIT_v2";
const BASE_ID: &str = "BASELINE_000_BEFORE_BUG";

/// Flush policy:
/// - Boottrace is buffered and flushed at most every ~250ms or when buffer grows large.
/// - A "forced" flush is rate-limited (min 50ms) to keep SD I/O from killing FPS.
/// - last_stage is updated in memory every call, but only written to SD at most every ~250ms
///   (or forced), keeping hangs debuggable without per-frame FS churn.
const FLUSH_MS: u64 = 250;
const FORCE_FLUSH_MIN_MS: u64 = 50;
const STAGE_FLUSH_MS: u64 = 250;
const BOOTTRACE_BUF_MAX: usize = 2048;
const CONSOLE_QUEUE_MAX: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level { Info, Warn, Error }

struct RunLog {
    run_dir: String,
    boottrace_path: String,
    last_stage_path: String,
    status_path: String,
    warnings_path: String,

    seq: u64,
    verbosity: u8, // 0=off, 1=important only, 2=verbose

    boottrace: BufWriter<std::fs::File>,
    warnings: BufWriter<std::fs::File>,

    // buffered boottrace pending (reduces write calls)
    bt_buf: String,
    last_flush_ms: u64,
    last_force_flush_ms: u64,

    // last stage (memory) + periodic file update
    last_stage: String,
    last_stage_frame: u64,
    last_stage_flush_ms: u64,

    // console ring buffer of important lines for C HUD
    console_q: VecDeque<String>,
}

static RUNLOG: OnceLock<Mutex<RunLog>> = OnceLock::new();

pub fn build_id() -> &'static str { BUILD_ID }
pub fn base_id() -> &'static str { BASE_ID }

fn now_ms() -> u64 {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    dur.as_millis() as u64
}

fn ensure_dir(p: &str) -> bool {
    if p.is_empty() { return false; }
    let _ = fs::create_dir_all(p);
    Path::new(p).exists()
}

fn open_append(path: &str) -> Option<std::fs::File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn write_all_unbuffered(path: &str, data: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).write(true).truncate(true).open(path) {
        let _ = f.write_all(data.as_bytes());
        let _ = f.flush();
    }
}

fn swf_basename(p: &str) -> String {
    // keep filename; if none, fallback to "unknown.swf"
    let mut s = p.to_string();
    // strip URL prefixes
    if let Some(rest) = s.strip_prefix("file:///") { s = rest.to_string(); }
    else if let Some(rest) = s.strip_prefix("file://") { s = rest.to_string(); }
    // basename
    s.rsplit('/').next().unwrap_or("unknown.swf").to_string()
}

fn pick_run_dir(root_path: &str) -> String {
    // Primary: keep runs next to your SWFs folder (what you requested in the protocol)
    let swf = swf_basename(root_path);
    let primary = format!("sdmc:/flash/_runs/{}/{}", BUILD_ID, swf);
    if ensure_dir(&primary) { return primary; }

    // Fallback: common place users check for homebrew artifacts
    let fallback = format!("sdmc:/3ds/ruffle3ds_runs/{}/{}", BUILD_ID, swf);
    let _ = ensure_dir("sdmc:/3ds/ruffle3ds_runs");
    if ensure_dir(&fallback) { return fallback; }

    // Last resort: root
    let root = format!("sdmc:/ruffle3ds_runs/{}/{}", BUILD_ID, swf);
    let _ = ensure_dir("sdmc:/ruffle3ds_runs");
    let _ = ensure_dir(&root);
    root
}

pub fn init_for_swf(root_path: &str) {
    // Don't re-init if already set (one runlog per SWF launch)
    if RUNLOG.get().is_some() { return; }

    let run_dir = pick_run_dir(root_path);
    let boottrace_path = format!("{}/boottrace.txt", run_dir);
    let last_stage_path = format!("{}/last_stage.txt", run_dir);
    let status_path = format!("{}/status_snapshot.txt", run_dir);
    let warnings_path = format!("{}/warnings.txt", run_dir);

    let boottrace_file = open_append(&boottrace_path).unwrap();
    let warnings_file = open_append(&warnings_path).unwrap();

    let mut rl = RunLog {
        run_dir: run_dir.clone(),
        boottrace_path: boottrace_path.clone(),
        last_stage_path: last_stage_path.clone(),
        status_path: status_path.clone(),
        warnings_path: warnings_path.clone(),
        seq: 0,
        verbosity: 1,
        boottrace: BufWriter::new(boottrace_file),
        warnings: BufWriter::new(warnings_file),
        bt_buf: String::new(),
        last_flush_ms: 0,
        last_force_flush_ms: 0,
        last_stage: "init".to_string(),
        last_stage_frame: 0,
        last_stage_flush_ms: 0,
        console_q: VecDeque::new(),
    };

    // Build info + pointer file to quickly find the run folder
    let build_info_path = format!("{}/build_info.txt", rl.run_dir);
    let info = format!(
        "build_id={}\nbase_id={}\nstart_ms={}\nswf_path={}\nrun_dir={}\n",
        BUILD_ID, BASE_ID, now_ms(), root_path, rl.run_dir
    );
    write_all_unbuffered(&build_info_path, &info);

    // A "last run" pointer file (so you don't have to hunt for the run folder)
    write_all_unbuffered("sdmc:/flash/_runs/LAST_RUN.txt", &format!("{}\n", rl.run_dir));
    write_all_unbuffered("sdmc:/3ds/ruffle3ds_last_run.txt", &format!("{}\n", rl.run_dir));

    // Seed last stage
    write_all_unbuffered(&last_stage_path, "frame=0 stage=init\n");

    rl.last_flush_ms = now_ms();
    rl.last_stage_flush_ms = rl.last_flush_ms;

    let _ = RUNLOG.set(Mutex::new(rl));

    log_important(&format!("RunLog init ok run_dir={}", run_dir));
}

fn push_console(rl: &mut RunLog, line: &str) {
    // Keep console output minimal by default (verbosity=1).
    // Verbosity 1: warnings/errors + major stage lines.
    // Verbosity 2: also include shape/tess events (if callers log them).
    if rl.console_q.len() >= CONSOLE_QUEUE_MAX {
        rl.console_q.pop_front();
    }
    rl.console_q.push_back(line.to_string());
}

fn maybe_flush(rl: &mut RunLog, force: bool) {
    let now = now_ms();
    let due = now.saturating_sub(rl.last_flush_ms) >= FLUSH_MS || rl.bt_buf.len() >= BOOTTRACE_BUF_MAX;
    let force_ok = now.saturating_sub(rl.last_force_flush_ms) >= FORCE_FLUSH_MIN_MS;

    if due || (force && force_ok) {
        if !rl.bt_buf.is_empty() {
            let _ = rl.boottrace.write_all(rl.bt_buf.as_bytes());
            rl.bt_buf.clear();
        }
        let _ = rl.boottrace.flush();
        rl.last_flush_ms = now;
        if force { rl.last_force_flush_ms = now; }
    }
}

fn maybe_flush_stage(rl: &mut RunLog, force: bool) {
    let now = now_ms();
    let due = now.saturating_sub(rl.last_stage_flush_ms) >= STAGE_FLUSH_MS;
    if due || force {
        let data = format!("frame={} stage={}\n", rl.last_stage_frame, rl.last_stage);
        // Small file; do an unbuffered overwrite so it's always readable after a hang.
        write_all_unbuffered(&rl.last_stage_path, &data);
        rl.last_stage_flush_ms = now;
    }
}

fn log_impl(level: Level, msg: &str, important: bool) {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut rl) = lock.lock() {
            rl.seq = rl.seq.wrapping_add(1);
            let tag = match level { Level::Info => "INFO", Level::Warn => "WARN", Level::Error => "ERR " };
            let line = format!("[{:06}] {} {}\n", rl.seq, tag, msg);

            // Always store in boottrace if verbosity > 0
            if rl.verbosity > 0 {
                rl.bt_buf.push_str(&line);
            }

            // Warnings/errors also go to warnings.txt
            if level != Level::Info {
                let _ = rl.warnings.write_all(line.as_bytes());
                let _ = rl.warnings.flush();
            }

            // Console output: keep lightweight by default
            if rl.verbosity >= 2 || (rl.verbosity == 1 && (important || level != Level::Info)) {
                // Trim for console
                let mut s = line.trim_end().to_string();
                if s.len() > 60 { s.truncate(60); }
                push_console(&mut rl, &s);
            }

            maybe_flush(&mut rl, important);
        }
    }
}

pub fn log_line(msg: &str) { log_impl(Level::Info, msg, false); }
pub fn log_important(msg: &str) { log_impl(Level::Info, msg, true); }
pub fn warn_line(msg: &str) { log_impl(Level::Warn, msg, true); }
pub fn error_line(msg: &str) { log_impl(Level::Error, msg, true); }

/// Update current stage for hang diagnosis.
/// This updates memory every call; SD write is rate-limited and can be forced.
pub fn stage(stage: &str, frame: u64) {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut rl) = lock.lock() {
            rl.last_stage = stage.to_string();
            rl.last_stage_frame = frame;
            // Only force stage flush if we're entering a potentially-heavy phase.
            let force = stage.contains("tess") || stage.contains("earcut") || stage.contains("register_shape");
            maybe_flush_stage(&mut rl, force);
            // Also rate-limited boottrace flush when entering heavy work.
            if force {
                maybe_flush(&mut rl, true);
            }
        }
    }
}

pub fn status_snapshot(text: &str) {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut rl) = lock.lock() {
            // Append snapshot line
            if let Some(mut f) = open_append(&rl.status_path) {
                rl.seq = rl.seq.wrapping_add(1);
                let line = format!("[{:06}] {}\n", rl.seq, text);
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
            log_important("status_snapshot written");
        }
    }
}

/// Drain pending console lines into `out` as newline separated UTF-8.
/// Returns number of bytes written.
pub fn drain_console(out: &mut [u8]) -> usize {
    if out.is_empty() { return 0; }
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut rl) = lock.lock() {
            let mut written = 0usize;
            while let Some(line) = rl.console_q.pop_front() {
                let bytes = line.as_bytes();
                if written + bytes.len() + 1 > out.len() { // + '\n'
                    // Put it back if it doesn't fit
                    rl.console_q.push_front(line);
                    break;
                }
                out[written..written+bytes.len()].copy_from_slice(bytes);
                written += bytes.len();
                out[written] = b'\n';
                written += 1;
            }
            return written;
        }
    }
    0
}

pub fn set_verbosity(level: u8) {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut rl) = lock.lock() {
            rl.verbosity = level.min(2);
            // Echo to both console and file
            let msg = format!("runlog verbosity={}", rl.verbosity);
            rl.seq = rl.seq.wrapping_add(1);
            let seq = rl.seq;
            rl.bt_buf.push_str(&format!("[{:06}] INFO {}\n", seq, msg));
            push_console(&mut rl, &format!("INFO {}", msg));
            maybe_flush(&mut rl, true);
        }
    }
}


/// Current runlog verbosity (0..2).
pub fn get_verbosity() -> u8 {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(rl) = lock.lock() {
            return rl.verbosity;
        }
    }
    0
}

pub fn is_verbose() -> bool { get_verbosity() >= 2 }

pub fn cycle_verbosity() {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(rl) = lock.lock() {
            let next = (rl.verbosity + 1) % 3;
            drop(rl);
            set_verbosity(next);
        }
    }
}

pub fn shutdown() {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut rl) = lock.lock() {
            maybe_flush(&mut rl, true);
            let _ = rl.warnings.flush();
            maybe_flush_stage(&mut rl, true);
        }
    }
}

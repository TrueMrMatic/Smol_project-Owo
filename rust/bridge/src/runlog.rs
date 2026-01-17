use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use core::fmt::Write as FmtWrite;

const BUILD_ID: &str = "PATCH_010_STEP3_SOLID_COLOR";
const BASE_ID: &str = "PATCH_008_TEXT_VECTOR";

/// Flush policy:
/// - Boottrace is buffered and flushed at most every ~250ms or when buffer grows large.
/// - A "forced" flush is rate-limited (min 50ms) to keep SD I/O from killing FPS.
/// - last_stage is updated in memory every call, but only written to SD at most every ~250ms
///   (or forced), keeping hangs debuggable without per-frame FS churn.
const FLUSH_MS: u64 = 250;
const FORCE_FLUSH_MIN_MS: u64 = 50;
const STAGE_FLUSH_MS: u64 = 250;
const STATUS_FLUSH_MS: u64 = 200;
const BOOTTRACE_BUF_MAX: usize = 2048;
const CONSOLE_QUEUE_MAX: usize = 64;
const RECENT_WARNINGS_MAX: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level { Info, Warn, Error }

struct RunLog {
    swf_name: String,
    run_dir: String,
    boottrace_path: String,
    last_stage_path: String,
    status_path: String,
    warnings_path: String,

    seq: u64,
    verbosity: u8, // 0=off, 1=important only, 2=verbose

    boottrace: BufWriter<std::fs::File>,
    status: BufWriter<std::fs::File>,
    warnings: BufWriter<std::fs::File>,

    // buffered boottrace pending (reduces write calls)
    bt_buf: String,
    last_flush_ms: u64,
    last_force_flush_ms: u64,

    // last stage (memory) + periodic file update
    last_stage: String,
    last_stage_frame: u64,
    last_stage_flush_ms: u64,
    stage_pending: bool,
    stage_force: bool,

    // deferred status snapshots to avoid blocking input/UI
    status_q: VecDeque<String>,
    last_status_flush_ms: u64,

    // console ring buffer of important lines for C HUD
    console_q: VecDeque<String>,
    recent_warnings: VecDeque<String>,
}

static RUNLOG: OnceLock<Mutex<Option<RunLog>>> = OnceLock::new();

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
    let timestamp = now_ms();
    let primary = format!("sdmc:/flash/_runs/{}/{}_{}", BUILD_ID, timestamp, swf);
    let _ = ensure_dir(&primary);
    primary
}

pub fn init_for_swf(root_path: &str) {
    let swf_name = swf_basename(root_path);
    let run_dir = pick_run_dir(root_path);
    let boottrace_path = format!("{}/boottrace.txt", run_dir);
    let last_stage_path = format!("{}/last_stage.txt", run_dir);
    let status_path = format!("{}/status_snapshot.txt", run_dir);
    let warnings_path = format!("{}/warnings.txt", run_dir);

    let boottrace_file = open_append(&boottrace_path).unwrap();
    let status_file = open_append(&status_path).unwrap();
    let warnings_file = open_append(&warnings_path).unwrap();

    let mut rl = RunLog {
        swf_name: swf_name.clone(),
        run_dir: run_dir.clone(),
        boottrace_path: boottrace_path.clone(),
        last_stage_path: last_stage_path.clone(),
        status_path: status_path.clone(),
        warnings_path: warnings_path.clone(),
        seq: 0,
        verbosity: 2,
        boottrace: BufWriter::new(boottrace_file),
        status: BufWriter::new(status_file),
        warnings: BufWriter::new(warnings_file),
        bt_buf: String::new(),
        last_flush_ms: 0,
        last_force_flush_ms: 0,
        last_stage: "init".to_string(),
        last_stage_frame: 0,
        last_stage_flush_ms: 0,
        stage_pending: false,
        stage_force: false,
        status_q: VecDeque::new(),
        last_status_flush_ms: 0,
        console_q: VecDeque::new(),
        recent_warnings: VecDeque::new(),
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

    // Seed last stage
    write_all_unbuffered(&last_stage_path, "frame=0 stage=init\n");

    rl.last_flush_ms = now_ms();
    rl.last_stage_flush_ms = rl.last_flush_ms;

    let lock = RUNLOG.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        if let Some(existing) = guard.as_ref() {
            if existing.swf_name == swf_name {
                return;
            }
        }
        if let Some(mut existing) = guard.take() {
            shutdown_locked(&mut existing);
        }
        *guard = Some(rl);
    }

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

fn push_recent_warning(rl: &mut RunLog, line: &str) {
    if rl.recent_warnings.len() >= RECENT_WARNINGS_MAX {
        rl.recent_warnings.pop_front();
    }
    rl.recent_warnings.push_back(line.to_string());
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
        if let Ok(mut guard) = lock.lock() {
            let Some(rl) = guard.as_mut() else {
                return;
            };
            rl.seq = rl.seq.wrapping_add(1);
            let tag = match level { Level::Info => "INFO", Level::Warn => "WARN", Level::Error => "ERR " };

            // Always store in boottrace if verbosity > 0
            if rl.verbosity > 0 {
                let _ = writeln!(&mut rl.bt_buf, "[{:06}] {} {}", rl.seq, tag, msg);
            }

            // Warnings/errors also go to warnings.txt
            if level != Level::Info {
                let _ = writeln!(rl.warnings, "[{:06}] {} {}", rl.seq, tag, msg);
                let _ = rl.warnings.flush();
                let mut warning_line = String::new();
                let _ = write!(&mut warning_line, "[{:06}] {} {}", rl.seq, tag, msg);
                push_recent_warning(rl, warning_line.trim_end());
            }

            // Console output: keep lightweight by default
            if rl.verbosity >= 2 || (rl.verbosity == 1 && (important || level != Level::Info)) {
                // Trim for console
                let mut s = String::with_capacity(60);
                let _ = write!(&mut s, "[{:06}] {} {}", rl.seq, tag, msg);
                if s.len() > 60 { s.truncate(60); }
                push_console(rl, &s);
            }

            maybe_flush(rl, important);
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
        if let Ok(mut guard) = lock.lock() {
            let Some(rl) = guard.as_mut() else {
                return;
            };
            // Avoid per-frame allocations: stage() is called every frame, so reuse buffer storage.
            if rl.last_stage == stage {
                rl.last_stage_frame = frame;
            } else {
                rl.last_stage.clear();
                rl.last_stage.push_str(stage);
                rl.last_stage_frame = frame;
            }
            // Only force stage flush if we're entering a potentially-heavy phase.
            let force = stage.contains("tess") || stage.contains("earcut");
            rl.stage_pending = true;
            rl.stage_force = rl.stage_force || force;
            // Also rate-limited boottrace flush when entering heavy work.
            if force {
                maybe_flush(rl, true);
            }
        }
    }
}

pub fn status_snapshot(text: &str) {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut guard) = lock.lock() {
            let Some(rl) = guard.as_mut() else {
                return;
            };
            if rl.status_q.len() < 16 {
                rl.status_q.push_back(text.to_string());
            } else {
                rl.status_q.pop_front();
                rl.status_q.push_back(text.to_string());
            }
        }
    }
}

/// Flush deferred status snapshots without blocking input/UI.
pub fn tick() {
    let mut stage_write: Option<(String, String)> = None;
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut guard) = lock.lock() {
            let Some(rl) = guard.as_mut() else {
                return;
            };
            let now = now_ms();
            if rl.stage_pending {
                let due = now.saturating_sub(rl.last_stage_flush_ms) >= STAGE_FLUSH_MS;
                if due || rl.stage_force {
                    let data = format!("frame={} stage={}\n", rl.last_stage_frame, rl.last_stage);
                    stage_write = Some((rl.last_stage_path.clone(), data));
                    rl.last_stage_flush_ms = now;
                    rl.stage_pending = false;
                    rl.stage_force = false;
                }
            }

            if !rl.status_q.is_empty() && now.saturating_sub(rl.last_status_flush_ms) >= STATUS_FLUSH_MS {
                if let Some(text) = rl.status_q.pop_front() {
                    rl.seq = rl.seq.wrapping_add(1);
                    let line = format!("[{:06}] {}\n", rl.seq, text);
                    let _ = rl.status.write_all(line.as_bytes());
                    let _ = rl.status.flush();
                    rl.last_status_flush_ms = now;
                }
            }
        }
    }
    if let Some((path, data)) = stage_write {
        write_all_unbuffered(&path, &data);
    }
}

/// Drain pending console lines into `out` as newline separated UTF-8.
/// Returns number of bytes written.
pub fn drain_console(out: &mut [u8]) -> usize {
    if out.is_empty() { return 0; }
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut guard) = lock.lock() {
            let Some(rl) = guard.as_mut() else {
                return 0;
            };
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
        if let Ok(mut guard) = lock.lock() {
            let Some(rl) = guard.as_mut() else {
                return;
            };
            let requested = level.min(2);
            rl.verbosity = 2;
            // Echo to both console and file
            let msg = format!("runlog verbosity={} (requested={})", rl.verbosity, requested);
            rl.seq = rl.seq.wrapping_add(1);
            let seq = rl.seq;
            rl.bt_buf.push_str(&format!("[{:06}] INFO {}\n", seq, msg));
            push_console(rl, &format!("INFO {}", msg));
            maybe_flush(rl, true);
        }
    }
}


/// Current runlog verbosity (0..2).
pub fn get_verbosity() -> u8 {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(guard) = lock.lock() {
            if let Some(rl) = guard.as_ref() {
                return rl.verbosity;
            }
        }
    }
    0
}

pub fn is_verbose() -> bool { get_verbosity() >= 2 }

#[derive(Clone, Debug)]
pub struct RunlogSnapshot {
    pub last_stage: String,
    pub last_stage_frame: u64,
    pub recent_warnings: Vec<String>,
}

pub fn snapshot_info() -> Option<RunlogSnapshot> {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(guard) = lock.lock() {
            if let Some(rl) = guard.as_ref() {
                return Some(RunlogSnapshot {
                    last_stage: rl.last_stage.clone(),
                    last_stage_frame: rl.last_stage_frame,
                    recent_warnings: rl.recent_warnings.iter().cloned().collect(),
                });
            }
        }
    }
    None
}

pub fn cycle_verbosity() {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(guard) = lock.lock() {
            drop(guard);
            set_verbosity(2);
        }
    }
}

fn shutdown_locked(rl: &mut RunLog) {
    maybe_flush(rl, true);
    while let Some(text) = rl.status_q.pop_front() {
        rl.seq = rl.seq.wrapping_add(1);
        let line = format!("[{:06}] {}\n", rl.seq, text);
        let _ = rl.status.write_all(line.as_bytes());
    }
    let _ = rl.status.flush();
    let _ = rl.warnings.flush();
    maybe_flush_stage(rl, true);
}

pub fn shutdown() {
    if let Some(lock) = RUNLOG.get() {
        if let Ok(mut guard) = lock.lock() {
            if let Some(mut rl) = guard.take() {
                shutdown_locked(&mut rl);
            }
        }
    }
}

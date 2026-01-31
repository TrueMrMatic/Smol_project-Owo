use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

const BUILD_ID: &str = "RUNLOG_DISABLED";
const BASE_ID: &str = "RUNLOG_DISABLED";
const CONSOLE_QUEUE_MAX: usize = 64;
const RECENT_WARNINGS_MAX: usize = 8;

#[derive(Clone, Debug)]
pub struct RunlogSnapshot {
    pub last_stage: String,
    pub last_stage_frame: u64,
    pub recent_warnings: Vec<String>,
}

struct RunlogStub {
    console_q: VecDeque<String>,
    recent_warnings: VecDeque<String>,
    verbosity: u8,
    last_stage: String,
    last_stage_frame: u64,
}

static RUNLOG: OnceLock<Mutex<RunlogStub>> = OnceLock::new();

fn with_runlog<T>(f: impl FnOnce(&mut RunlogStub) -> T) -> T {
    let lock = RUNLOG.get_or_init(|| {
        Mutex::new(RunlogStub {
            console_q: VecDeque::new(),
            recent_warnings: VecDeque::new(),
            verbosity: 1,
            last_stage: String::new(),
            last_stage_frame: 0,
        })
    });
    let mut guard = lock.lock().unwrap();
    f(&mut guard)
}

pub fn build_id() -> &'static str { BUILD_ID }
pub fn base_id() -> &'static str { BASE_ID }

pub fn init_for_swf(_root_path: &str) {
    with_runlog(|rl| {
        rl.console_q.clear();
        rl.recent_warnings.clear();
        rl.last_stage.clear();
        rl.last_stage_frame = 0;
    });
}

pub fn log_line(msg: &str) { log_impl(msg, false); }
pub fn log_important(msg: &str) { log_impl(msg, true); }
pub fn warn_line(msg: &str) {
    log_impl(msg, true);
    with_runlog(|rl| {
        if rl.recent_warnings.len() >= RECENT_WARNINGS_MAX {
            rl.recent_warnings.pop_front();
        }
        rl.recent_warnings.push_back(msg.to_string());
    });
}
pub fn error_line(msg: &str) { log_impl(msg, true); }

fn log_impl(msg: &str, important: bool) {
    with_runlog(|rl| {
        if rl.verbosity == 0 {
            return;
        }
        if !important && rl.verbosity < 2 {
            return;
        }
        if rl.console_q.len() >= CONSOLE_QUEUE_MAX {
            rl.console_q.pop_front();
        }
        rl.console_q.push_back(msg.to_string());
    });
}

pub fn stage(stage: &str, frame: u64) {
    with_runlog(|rl| {
        rl.last_stage = stage.to_string();
        rl.last_stage_frame = frame;
    });
}

pub fn status_snapshot(_text: &str) {}

pub fn tick() {}

pub fn drain_console(out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    with_runlog(|rl| {
        let mut written = 0usize;
        while let Some(line) = rl.console_q.pop_front() {
            let bytes = line.as_bytes();
            let needed = bytes.len().saturating_add(1);
            if written + needed > out.len() {
                rl.console_q.push_front(line);
                break;
            }
            out[written..written + bytes.len()].copy_from_slice(bytes);
            written += bytes.len();
            if written < out.len() {
                out[written] = b'\n';
                written += 1;
            }
        }
        written
    })
}

pub fn set_verbosity(level: u8) {
    with_runlog(|rl| {
        rl.verbosity = level.min(2);
    });
}

pub fn get_verbosity() -> u8 {
    with_runlog(|rl| rl.verbosity)
}

pub fn is_verbose() -> bool { get_verbosity() >= 2 }

pub fn snapshot_info() -> Option<RunlogSnapshot> {
    Some(with_runlog(|rl| RunlogSnapshot {
        last_stage: rl.last_stage.clone(),
        last_stage_frame: rl.last_stage_frame,
        recent_warnings: rl.recent_warnings.iter().cloned().collect(),
    }))
}

pub fn cycle_verbosity() {
    with_runlog(|rl| {
        rl.verbosity = 2;
    });
}

pub fn shutdown() {}

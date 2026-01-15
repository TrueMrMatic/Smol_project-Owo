use std::sync::OnceLock;

const CONFIG_PATH: &str = "sdmc:/flash/renderer.cfg";

#[derive(Debug, Clone, Copy)]
pub struct RenderConfig {
    pub textured_bitmaps: bool,
    pub masks_enabled: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self { textured_bitmaps: true, masks_enabled: true }
    }
}

static CONFIG: OnceLock<RenderConfig> = OnceLock::new();

pub fn render_config() -> &'static RenderConfig {
    CONFIG.get_or_init(read_config)
}

pub fn textured_bitmaps_enabled() -> bool {
    render_config().textured_bitmaps
}

pub fn masks_enabled() -> bool {
    render_config().masks_enabled
}

fn read_config() -> RenderConfig {
    let mut cfg = RenderConfig::default();
    let text = match std::fs::read_to_string(CONFIG_PATH) {
        Ok(text) => text,
        Err(_) => return cfg,
    };

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if key.eq_ignore_ascii_case("textured_bitmaps") {
            cfg.textured_bitmaps = matches!(
                value,
                "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES"
            );
        }
        if key.eq_ignore_ascii_case("masks_enabled") {
            cfg.masks_enabled = matches!(
                value,
                "1" | "true" | "TRUE" | "on" | "ON" | "yes" | "YES"
            );
        }
    }

    cfg
}

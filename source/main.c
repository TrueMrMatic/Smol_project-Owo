#include <3ds.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "file_selector.h"

// Rust exports
void* bridge_player_create_with_url(const char* url);
void  bridge_tick(void* ctx);
void  bridge_player_destroy(void* ctx);

// Run bundle / protocol helpers (exported by Rust)
void  bridge_write_status_snapshot_ctx(void* ctx);

// Renderer controls (exported by Rust)
u32   bridge_runlog_drain(char* out, u32 out_len);
void  bridge_runlog_cycle_verbosity(void);
void   bridge_print_status(void* ctx);
void   bridge_request_command_dump_ctx(void* ctx);
void   bridge_toggle_shape_mode_ctx(void* ctx);
void   bridge_toggle_wireframe_once_ctx(void* ctx);
void   bridge_set_wireframe_hold_ctx(void* ctx, int enabled);
uint32_t bridge_renderer_ready_ctx(void* ctx);
size_t bridge_get_status_text(void* ctx, char* out, size_t cap);

static void clear_top_black_double(void) {
    // Clear BOTH top-screen buffers to avoid flicker when the selector swaps buffers.
    for (int i = 0; i < 2; i++) {
        u16 w=0, h=0;
        u8* fb = gfxGetFramebuffer(GFX_TOP, GFX_LEFT, &w, &h);
        if (fb && w && h) {
            memset(fb, 0, (size_t)w * (size_t)h * 3);
        }
        gfxFlushBuffers();
        gfxSwapBuffers();
        gspWaitForVBlank();
    }
}

// ---- Bottom-screen UI layout ----
#define UI_ROW_TITLE 1
#define UI_ROW_SWF 2
#define UI_ROW_CONTROLS 3
#define UI_ROW_LOG_LABEL 8
#define UI_ROW_LOG_START 9
#define UI_LOG_LINES 16
#define UI_ROW_NOTICE 27
#define UI_ROW_WARN 28
#define UI_ROW_HUD 29

static const char* path_basename(const char* path) {
    const char* last = path;
    for (const char* p = path; *p; ++p) {
        if (*p == '/' || *p == '\\') {
            last = p + 1;
        }
    }
    return last;
}

static void ui_clear_log_window(void);

static void ui_draw_static(const char* swf_path) {
    consoleClear();
    const char* base = path_basename(swf_path);
    printf("\x1b[%d;0HRuffle3DS Player", UI_ROW_TITLE);
    printf("\x1b[%d;0HSWF: %-36.36s", UI_ROW_SWF, base);
    printf("\x1b[%d;0HControls:", UI_ROW_CONTROLS);
    printf("\x1b[%d;0H  X: shape mode  L: wireframe", UI_ROW_CONTROLS + 1);
    printf("\x1b[%d;0H  Y: write snapshot", UI_ROW_CONTROLS + 2);
    printf("\x1b[%d;0H  SELECT: verbosity  B: back", UI_ROW_CONTROLS + 3);
    printf("\x1b[%d;0H  START: exit app", UI_ROW_CONTROLS + 4);
    printf("\x1b[%d;0HLogs:", UI_ROW_LOG_LABEL);
    ui_clear_log_window();
}

// ---- Boottrace console window (bottom screen) ----
#define LOG_WINDOW_LINES UI_LOG_LINES
static char g_log_lines[LOG_WINDOW_LINES][41];
static int  g_log_count = 0;
static int  g_log_start = 0;
static int  g_log_dirty = 1;
static char g_notice[41] = {0};
static u32 g_notice_ttl = 0;
static u64 g_last_snapshot_ms = 0;

static void ui_clear_log_window(void) {
    for (int i = 0; i < LOG_WINDOW_LINES; i++) {
        int row = UI_ROW_LOG_START + i;
        printf("\x1b[%d;0H%-40s", row, "");
    }
}

static void ui_reset_log_state(void) {
    g_log_count = 0;
    g_log_start = 0;
    g_log_dirty = 1;
    g_notice[0] = 0;
    g_notice_ttl = 0;
    ui_clear_log_window();
}

static void ui_set_notice(const char* msg, u32 ttl_frames) {
    if (!msg) return;
    strncpy(g_notice, msg, 40);
    g_notice[40] = 0;
    g_notice_ttl = ttl_frames;
    printf("\x1b[%d;0H%-40s", UI_ROW_NOTICE, g_notice);
}

static void log_push_line(const char* s) {
    if (!s || !s[0]) return;
    // copy up to 40 chars
    char tmp[41];
    strncpy(tmp, s, 40);
    tmp[40] = 0;

    int idx = (g_log_start + g_log_count) % LOG_WINDOW_LINES;
    if (g_log_count < LOG_WINDOW_LINES) {
        g_log_count++;
    } else {
        g_log_start = (g_log_start + 1) % LOG_WINDOW_LINES;
        idx = (g_log_start + g_log_count - 1) % LOG_WINDOW_LINES;
    }
    strncpy(g_log_lines[idx], tmp, 41);
    g_log_lines[idx][40] = 0;
    g_log_dirty = 1;
}

static void log_drain_from_rust(void) {
    // Drain a few lines per frame to avoid slowing down loading.
    char buf[512];
    memset(buf, 0, sizeof(buf));
    u32 n = bridge_runlog_drain(buf, (u32)sizeof(buf));
    if (n == 0) return;
    // Split by newline
    const char* p = buf;
    while (*p) {
        const char* nl = strchr(p, '\n');
        if (!nl) {
            log_push_line(p);
            break;
        }
        char line[96];
        size_t len = (size_t)(nl - p);
        if (len > sizeof(line) - 1) len = sizeof(line) - 1;
        memcpy(line, p, len);
        line[len] = 0;
        log_push_line(line);
        p = nl + 1;
    }
}

static void log_redraw_window(void) {
    if (!g_log_dirty) return;
    g_log_dirty = 0;
    // Draw log window in rows UI_ROW_LOG_START..(UI_ROW_LOG_START+LOG_WINDOW_LINES-1)
    for (int i = 0; i < LOG_WINDOW_LINES; i++) {
        int row = UI_ROW_LOG_START + i;
        char* line = (char*)"";
        if (i < g_log_count) {
            int idx = (g_log_start + i) % LOG_WINDOW_LINES;
            line = g_log_lines[idx];
        }
        printf("\x1b[%d;0H%-40s", row, line);
    }
}

static void hud_draw(void* ctx) {
    // FPS counter (smoothed over a short window).
    static u64 last_ms = 0;
    static u64 acc_ms = 0;
    static u32 acc_frames = 0;
    static u32 fps = 0;

    u64 now_ms = osGetTime();
    if (last_ms == 0) {
        last_ms = now_ms;
    }
    u64 dt_ms = now_ms - last_ms;
    last_ms = now_ms;

    // Cap dt to avoid giant spikes when resuming from pauses.
    if (dt_ms > 200) dt_ms = 200;

    acc_ms += dt_ms;
    acc_frames++;
    if (acc_ms >= 500) {
        // fps = frames / seconds
        // fps = acc_frames * 1000 / acc_ms
        fps = (u32)((acc_frames * 1000ULL) / (acc_ms ? acc_ms : 1));
        acc_ms = 0;
        acc_frames = 0;
    }

    char status[128];
    status[0] = 0;
    if (ctx) {
        bridge_get_status_text(ctx, status, sizeof(status));
    } else {
        strncpy(status, "IDLE", sizeof(status) - 1);
        status[sizeof(status) - 1] = 0;
    }

    // If Rust prefixed a warning with "!xxx", show it one row above the main HUD.
    // This avoids the warning being cropped at the end of the 40-char status line.
    char warn[48] = {0};
    const char* status_rest = status;
    if (status[0] == '!') {
        const char* sp = strchr(status, ' ');
        if (sp) {
            size_t n = (size_t)(sp - status);
            if (n > sizeof(warn) - 1) n = sizeof(warn) - 1;
            // Drop the leading '!' for display.
            if (n > 1) {
                strncpy(warn, status + 1, n - 1);
                warn[n - 1] = 0;
            }
            status_rest = sp + 1;
        } else {
            // Entire status is a warning token.
            strncpy(warn, status + 1, sizeof(warn) - 1);
            warn[sizeof(warn) - 1] = 0;
            status_rest = "";
        }
    }

    // Compose one 40-char line. Keep it stable to avoid flicker.
    char line[128];
    u32 fps_clamped = fps;
    if (fps_clamped > 99) fps_clamped = 99;
    snprintf(line, sizeof(line), "FPS:%02lu %s", (unsigned long)fps_clamped, status_rest);

    // Print at fixed rows to avoid scrolling/flicker.
    // Bottom console is 40x30 chars; row index is 1-based.
    if (g_notice_ttl > 0) {
        g_notice_ttl--;
        printf("\x1b[%d;0H%-40s", UI_ROW_NOTICE, g_notice);
    } else {
        printf("\x1b[%d;0H%-40s", UI_ROW_NOTICE, "");
    }
    printf("\x1b[%d;0H%-40s", UI_ROW_WARN, warn[0] ? warn : "");
    printf("\x1b[%d;0H%-40s", UI_ROW_HUD, line);
}

int main(int argc, char* argv[]) {
    (void)argc; (void)argv;

    gfxInitDefault();

    // Bottom console only: keep TOP screen free for graphics.
    PrintConsole conBot;
    consoleInit(GFX_BOTTOM, &conBot);
    consoleSelect(&conBot);

    while (aptMainLoop()) {
        char swf_path[1024];
        file_selector_clear_exit_request();
        bool ok = file_selector_pick_swf(swf_path, sizeof(swf_path));
        if (!ok) {
            if (file_selector_exit_requested()) goto exit_app;
            // Cancelled (B at root)
            consoleClear();
            printf("Cancelled. Press START to exit.\n");
            while (aptMainLoop()) {
                hidScanInput();
                if (hidKeysDown() & KEY_START) goto exit_app;
                gspWaitForVBlank();
            }
            goto exit_app;
        }

        consoleClear();
        printf("Selected: %s\n", swf_path);
        printf("Initializing Ruffle...\n");

        void* ctx = bridge_player_create_with_url(swf_path);

        ui_reset_log_state();
        ui_draw_static(swf_path);

        // Playback loop
        while (aptMainLoop()) {
            hidScanInput();
            u32 down = hidKeysDown();
            u32 held = hidKeysHeld();

            if (down & KEY_START) {
                bridge_player_destroy(ctx);
                goto exit_app;
            }

            if (down & KEY_B) {
                // Back to file selector
                bridge_player_destroy(ctx);
                clear_top_black_double();
                break;
            }

            if (down & KEY_SELECT) {
                bridge_runlog_cycle_verbosity();
            }

            if (down & KEY_X) {
                // Toggle shape mode: rectangle bounds vs triangle mesh mode.
                bridge_toggle_shape_mode_ctx(ctx);
            }

            // Hold L to show triangle edges continuously.
            bridge_set_wireframe_hold_ctx(ctx, (held & KEY_L) ? 1 : 0);
            if (down & KEY_Y) {
                // Write an SD snapshot so we can debug freezes later.
                u64 now_ms = osGetTime();
                if (now_ms - g_last_snapshot_ms >= 500) {
                    g_last_snapshot_ms = now_ms;
                    bridge_write_status_snapshot_ctx(ctx);
                    ui_set_notice("snapshot saved", 60);
                } else {
                    ui_set_notice("snapshot cooldown", 30);
                }
            }

            // Tick+Render
            bridge_tick(ctx);

            // Drain and display important boottrace lines (rate-limited).
            log_drain_from_rust();
            log_redraw_window();

            // HUD line (bottom screen)
            hud_draw(ctx);

            // Present the framebuffer we wrote in Rust.
            gfxFlushBuffers();
            gfxSwapBuffers();
            gspWaitForVBlank();
        }
    }

exit_app:
    gfxExit();
    return 0;
}

#include <sys/types.h>

// Rust (and some crates) require this on some targets for HashMap/random seeding.
// On a real app, use the 3DS secure RNG (sslc), but this is enough to link.
ssize_t getrandom(void *buf, size_t buflen, unsigned int flags) {
    (void)flags;
    static bool seeded = false;
    if (!seeded) { srand((unsigned)time(NULL)); seeded = true; }

    unsigned char *p = (unsigned char *)buf;
    for (size_t i = 0; i < buflen; i++) {
        p[i] = rand() & 0xFF;
    }
    return (ssize_t)buflen;
}

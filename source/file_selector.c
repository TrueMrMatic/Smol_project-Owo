#include "file_selector.h"

#include <3ds.h>
#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>


// --- selector state ---
static bool g_exit_requested = false;
static char g_last_cwd[512] = "sdmc:/";

bool file_selector_exit_requested(void) { return g_exit_requested; }
void file_selector_clear_exit_request(void) { g_exit_requested = false; }

#ifndef NAME_MAX
#define NAME_MAX 255
#endif

typedef struct {
    char name[NAME_MAX + 1];
    bool is_dir;
} Entry;

static bool has_swf_ext(const char* name) {
    const char* dot = strrchr(name, '.');
    if (!dot) return false;
    return (strcasecmp(dot, ".swf") == 0);
}

static bool is_directory(const char* full_path) {
    struct stat st;
    if (stat(full_path, &st) != 0) return false;
    return S_ISDIR(st.st_mode);
}

static int entry_cmp(const void* a, const void* b) {
    const Entry* ea = (const Entry*)a;
    const Entry* eb = (const Entry*)b;

    // Dirs first
    if (ea->is_dir != eb->is_dir) return ea->is_dir ? -1 : 1;
    return strcasecmp(ea->name, eb->name);
}

static void path_join(char* out, size_t cap, const char* dir, const char* name) {
    if (!out || cap == 0) return;

    const size_t dlen = strlen(dir);
    const bool need_slash = (dlen > 0 && dir[dlen - 1] != '/');

    int n = snprintf(out, cap, need_slash ? "%s/%s" : "%s%s", dir, name);
    if (n < 0 || (size_t)n >= cap) {
        // Truncated; keep it NUL-terminated.
        out[cap - 1] = '\0';
    }
}

static void path_parent(char* path) {
    size_t n = strlen(path);
    if (n == 0) return;

    // strip trailing '/'
    while (n > 0 && path[n - 1] == '/') {
        path[n - 1] = '\0';
        n--;
    }
    if (n == 0) {
        strcpy(path, "sdmc:/");
        return;
    }

    char* last = strrchr(path, '/');
    if (!last) {
        strcpy(path, "sdmc:/");
        return;
    }

    // keep the slash
    last[1] = '\0';

    // normalize root
    if (strcmp(path, "sdmc:") == 0) strcpy(path, "sdmc:/");
    if (strcmp(path, "") == 0) strcpy(path, "sdmc:/");
}

static bool list_dir(const char* dir, Entry** out_entries, size_t* out_count) {
    *out_entries = NULL;
    *out_count = 0;

    DIR* d = opendir(dir);
    if (!d) return false;

    size_t cap = 128;
    Entry* entries = (Entry*)malloc(sizeof(Entry) * cap);
    if (!entries) {
        closedir(d);
        return false;
    }

    struct dirent* ent;
    while ((ent = readdir(d)) != NULL) {
        const char* name = ent->d_name;
        if (!name || !name[0]) continue;
        if (strcmp(name, ".") == 0 || strcmp(name, "..") == 0) continue;

        char full[1024];
        path_join(full, sizeof(full), dir, name);

        bool dir_flag = is_directory(full);
        bool keep = dir_flag || has_swf_ext(name);
        if (!keep) continue;

        if (*out_count >= cap) {
            cap *= 2;
            Entry* grown = (Entry*)realloc(entries, sizeof(Entry) * cap);
            if (!grown) {
                free(entries);
                closedir(d);
                return false;
            }
            entries = grown;
        }

        strncpy(entries[*out_count].name, name, NAME_MAX);
        entries[*out_count].name[NAME_MAX] = '\0';
        entries[*out_count].is_dir = dir_flag;
        (*out_count)++;
    }

    closedir(d);

    qsort(entries, *out_count, sizeof(Entry), entry_cmp);
    *out_entries = entries;
    return true;
}

static void clamp_scroll(size_t count, size_t lines, size_t* sel, size_t* scroll) {
    if (count == 0) {
        *sel = 0;
        *scroll = 0;
        return;
    }
    if (*sel >= count) *sel = count - 1;
    if (*sel < *scroll) *scroll = *sel;
    if (*sel >= *scroll + lines) *scroll = *sel - lines + 1;
}

static void draw_ui(const char* cwd, const Entry* entries, size_t count, size_t lines, size_t sel, size_t scroll, bool list_ok) {
    consoleClear();

    // Hide cursor (prevents the little blinking cursor from distracting you)
    printf("\x1b[?25l");

    printf("SWF browser\n");
    printf("A: open/select   B: back/cancel   X: refresh\n");
    printf("D-Pad: move   L/R: page\n\n");
    printf("Dir: %s\n\n", cwd);

    if (!list_ok) {
        printf("Failed to open directory.\n");
        printf("Press B to go back/cancel, X to retry.\n");
        return;
    }

    if (count == 0) {
        printf("(no .swf files in this folder)\n");
        return;
    }

    for (size_t i = 0; i < lines; i++) {
        size_t idx = scroll + i;
        if (idx >= count) break;
        const Entry* e = &entries[idx];
        printf("%c %s%s\n",
               (idx == sel) ? '>' : ' ',
               e->name,
               e->is_dir ? "/" : "");
    }
}

static void free_entries(Entry** entries, size_t* count) {
    if (*entries) free(*entries);
    *entries = NULL;
    *count = 0;
}

// Ensure BOTH framebuffers contain the same console text.
// This prevents flicker when double-buffering swaps between buffers.
static void present_console_stable(void) {
    gfxFlushBuffers();
    gfxSwapBuffers();
    gspWaitForVBlank();

    // After swap, we're drawing into the other back buffer.
    // Caller should have already drawn the same UI; but for safety, caller can redraw.
}

bool file_selector_pick_swf(char* out_path, size_t out_cap) {
    if (!out_path || out_cap == 0) return false;
    out_path[0] = '\0';

    char cwd[512];
    // Start in the last directory we used (so returning from a SWF doesn't drop you back to root).
    strncpy(cwd, g_last_cwd, sizeof(cwd));
    cwd[sizeof(cwd) - 1] = '\0';
    if (cwd[0] == '\0') {
        strncpy(cwd, "sdmc:/", sizeof(cwd));
        cwd[sizeof(cwd) - 1] = '\0';
    }

    const size_t lines = 18;
    size_t sel = 0;
    size_t scroll = 0;

    Entry* entries = NULL;
    size_t count = 0;
    bool list_ok = false;

    // We redraw only when something changes (dirty).
    bool dirty = true;
    bool need_reload = true;

    while (aptMainLoop()) {
        // Reload directory only when needed (enter folder, go back, manual refresh).
        if (need_reload) {
            free_entries(&entries, &count);
            list_ok = list_dir(cwd, &entries, &count);
            clamp_scroll(count, lines, &sel, &scroll);
            dirty = true;
            need_reload = false;
        }

        // Input
        hidScanInput();
        u32 down = hidKeysDown();

        if (down & KEY_START) {
            // Request app exit from the selector.
            g_exit_requested = true;
            strncpy(g_last_cwd, cwd, sizeof(g_last_cwd));
            g_last_cwd[sizeof(g_last_cwd) - 1] = '\0';
            free_entries(&entries, &count);
            return false;
        }

        if (down & KEY_X) {
            need_reload = true;
        }

        if (down & KEY_DOWN) {
            if (count > 0 && sel + 1 < count) { sel++; dirty = true; }
        }
        if (down & KEY_UP) {
            if (count > 0 && sel > 0) { sel--; dirty = true; }
        }
        if (down & KEY_L) {
            if (count > 0) {
                size_t step = (sel > lines) ? lines : sel;
                sel -= step;
                dirty = true;
            }
        }
        if (down & KEY_R) {
            if (count > 0) {
                size_t step = lines;
                sel = (sel + step >= count) ? (count - 1) : (sel + step);
                dirty = true;
            }
        }

        if (down & KEY_B) {
            if (strcmp(cwd, "sdmc:/") == 0) {
                free_entries(&entries, &count);
                return false;
            }
            path_parent(cwd);
            sel = 0;
            scroll = 0;
            need_reload = true;
        }

        if (down & KEY_A) {
            if (!list_ok) {
                // allow retry
                need_reload = true;
            } else if (count > 0) {
                Entry chosen = entries[sel];
                char full[1024];
                path_join(full, sizeof(full), cwd, chosen.name);

                if (chosen.is_dir) {
                    // Ensure trailing slash
                    size_t n = strlen(full);
                    if (n + 1 < sizeof(full) && full[n - 1] != '/') {
                        full[n] = '/';
                        full[n + 1] = '\0';
                    }
                    strncpy(cwd, full, sizeof(cwd));
                    cwd[sizeof(cwd) - 1] = '\0';
                    sel = 0;
                    scroll = 0;
                    need_reload = true;
                } else {
                    // Selected SWF
                    strncpy(out_path, full, out_cap);
                    // Remember this directory for next time we open the selector.
                    strncpy(g_last_cwd, cwd, sizeof(g_last_cwd));
                    g_last_cwd[sizeof(g_last_cwd) - 1] = '\0';
                    out_path[out_cap - 1] = '\0';
                    free_entries(&entries, &count);
                    return true;
                }
            }
        }

        // Keep selection window sane.
        if (dirty) {
            clamp_scroll(count, lines, &sel, &scroll);

            // Draw UI into current back buffer.
            draw_ui(cwd, entries, count, lines, sel, scroll, list_ok);

            // Present once, wait for VBlank, then draw AGAIN into the other buffer
            // so that both buffers contain identical text (no flicker).
            present_console_stable();
            draw_ui(cwd, entries, count, lines, sel, scroll, list_ok);
            gfxFlushBuffers();

            dirty = false;
        }

        // Regular present cadence (swap each frame, but both buffers now match).
        gfxSwapBuffers();
        gspWaitForVBlank();
    }

    free_entries(&entries, &count);
    return false;
}

// --- File IO for Rust (used by NavigatorBackend::fetch) ---
// Returns 0 on success; allocates *out_ptr with malloc and must be freed with bridge_free_file.
int bridge_read_file(const char* path, unsigned char** out_ptr, size_t* out_len) {
    if (!path || !out_ptr || !out_len) return -1;
    *out_ptr = NULL;
    *out_len = 0;

    FILE* f = fopen(path, "rb");
    if (!f) return -2;

    if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return -3; }
    long sz = ftell(f);
    if (sz <= 0) { fclose(f); return -4; }
    if (fseek(f, 0, SEEK_SET) != 0) { fclose(f); return -5; }

    unsigned char* buf = (unsigned char*)malloc((size_t)sz);
    if (!buf) { fclose(f); return -6; }

    size_t rd = fread(buf, 1, (size_t)sz, f);
    fclose(f);

    if (rd != (size_t)sz) { free(buf); return -7; }

    *out_ptr = buf;
    *out_len = (size_t)sz;
    return 0;
}

void bridge_free_file(unsigned char* ptr, size_t len) {
    (void)len;
    free(ptr);
}

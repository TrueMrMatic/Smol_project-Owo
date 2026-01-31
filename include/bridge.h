#ifndef BRIDGE_H
#define BRIDGE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void* bridge_engine_t;

bridge_engine_t bridge_engine_create(const char* swf_path, int screen_w, int screen_h);
void bridge_engine_destroy(bridge_engine_t handle);
void bridge_engine_tick(bridge_engine_t handle, uint32_t dt_ms);
void bridge_engine_mouse_move(bridge_engine_t handle, int x, int y);
void bridge_engine_mouse_button(bridge_engine_t handle, int button, bool down);
void bridge_engine_key(bridge_engine_t handle, int keycode, bool down);
uint32_t bridge_engine_last_error(char* out, uint32_t out_len);

uint32_t bridge_runlog_drain(char* out, uint32_t out_len);
void bridge_print_status(bridge_engine_t handle);
void bridge_write_status_snapshot_ctx(bridge_engine_t handle);
void bridge_request_command_dump_ctx(bridge_engine_t handle);
uint32_t bridge_toggle_affine_debug_overlay_ctx(bridge_engine_t handle);
void bridge_toggle_wireframe_once_ctx(bridge_engine_t handle);
void bridge_set_wireframe_hold_ctx(bridge_engine_t handle, int enabled);
uint32_t bridge_renderer_ready_ctx(bridge_engine_t handle);
size_t bridge_get_status_text(bridge_engine_t handle, char* out, size_t cap);

#ifdef __cplusplus
}
#endif

#endif

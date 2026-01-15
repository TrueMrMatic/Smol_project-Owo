#pragma once
#include <stdbool.h>
#include <stddef.h>

// Simple console-based SWF browser for the 3DS.
// Returns true on selection (out_path filled), false if user cancels or requests exit.
bool file_selector_pick_swf(char* out_path, size_t out_cap);

// If the user pressed START in the selector, this becomes true.
// main.c can use it to exit the app cleanly.
bool file_selector_exit_requested(void);
void file_selector_clear_exit_request(void);

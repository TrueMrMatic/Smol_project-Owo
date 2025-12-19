#pragma once
#include <stdbool.h>
#include <stdint.h>

typedef struct {
    // Root timeline (top-level tags in the main SWF)
    uint32_t total_tags;
    uint32_t showframe_tags;

    // Control tags inside DefineSprite (tag 39)
    uint32_t sprite_count;
    uint32_t sprite_tags;
    uint32_t sprite_showframe_tags;

    // From FileAttributes (tag 69)
    bool has_file_attributes;
    bool use_as3;       // true => AVM2 / ActionScript 3
    bool use_network;
    bool has_metadata;
} SwfTagSummary;

// Scans tags and prints the first `print_first_n` tags (including tags inside sprites).
// Returns 0 on success.
int swf_scan_tags(const char* path, SwfTagSummary* out, int print_first_n);
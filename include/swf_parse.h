#pragma once
#include <stdint.h>

typedef struct {
    char signature[4];     // "FWS" or "CWS" or "ZWS"
    uint8_t version;
    uint32_t file_length;  // uncompressed full length from header

    // From FrameSize RECT (in pixels)
    int width_px;
    int height_px;

    // From FrameRate (8.8 fixed)
    float fps;

    // From FrameCount
    uint16_t frame_count;
} SwfHeader;

int swf_read_header(const char* path, SwfHeader* out);
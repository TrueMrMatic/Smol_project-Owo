#include "swf_tags.h"
#include <3ds.h>
#include <zlib.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint16_t u16le(const uint8_t* p) { return (uint16_t)p[0] | ((uint16_t)p[1] << 8); }
static uint32_t u32le(const uint8_t* p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

// Bitreader for FrameSize RECT (bit-packed, MSB-first within bytes).
// SWF stores bytes little-endian, but bit order inside a byte is big-endian. :contentReference[oaicite:2]{index=2}
typedef struct { const uint8_t* b; size_t n; size_t bit; } BR;
static uint32_t br_bits(BR* r, int k) {
    uint32_t v = 0;
    for (int i = 0; i < k; i++) {
        size_t by = r->bit >> 3;
        int bi = 7 - (int)(r->bit & 7);
        r->bit++;
        uint8_t x = (by < r->n) ? r->b[by] : 0;
        v = (v << 1) | ((x >> bi) & 1);
    }
    return v;
}
static void br_align(BR* r) { size_t m = r->bit & 7; if (m) r->bit += (8 - m); }

// Returns byte offset (from start of SWF file) where tags begin.
static int swf_tag_start_offset(const uint8_t* swf, size_t len, size_t* out_off) {
    if (len < 8 + 1) return -1;
    BR r = { swf + 8, len - 8, 0 };
    uint32_t nbits = br_bits(&r, 5);
    for (int i = 0; i < 4; i++) (void)br_bits(&r, (int)nbits);
    br_align(&r);

    size_t rect_bytes = (r.bit >> 3);
    size_t off = 8 + rect_bytes + 4; // FrameRate(UI16) + FrameCount(UI16)
    if (off > len) return -2;
    *out_off = off;
    return 0;
}

// Load SWF into memory as an *uncompressed* buffer.
// For CWS, inflate body into a buffer of size FileLength (from header).
static int swf_load_uncompressed(const char* path, uint8_t** out_buf, size_t* out_len, char sig_out[4]) {
    *out_buf = NULL; *out_len = 0;
    sig_out[0] = sig_out[1] = sig_out[2] = 0; sig_out[3] = 0;

    FILE* f = fopen(path, "rb");
    if (!f) return -1;

    uint8_t h8[8];
    if (fread(h8, 1, 8, f) != 8) { fclose(f); return -2; }

    char sig[4] = { (char)h8[0], (char)h8[1], (char)h8[2], 0 };
    uint8_t ver = h8[3];
    uint32_t file_len = u32le(&h8[4]);
    strcpy(sig_out, sig);

    const uint32_t MAX_SWF = 12u * 1024u * 1024u; // safety cap
    if (file_len < 8 || file_len > MAX_SWF) { fclose(f); return -3; }

    uint8_t* buf = (uint8_t*)malloc(file_len);
    if (!buf) { fclose(f); return -4; }

    // Normalize to "FWS-like" buffer for uniform parsing
    buf[0] = 'F'; buf[1] = 'W'; buf[2] = 'S';
    buf[3] = ver;
    buf[4] = (uint8_t)(file_len & 0xFF);
    buf[5] = (uint8_t)((file_len >> 8) & 0xFF);
    buf[6] = (uint8_t)((file_len >> 16) & 0xFF);
    buf[7] = (uint8_t)((file_len >> 24) & 0xFF);

    if (!strcmp(sig, "FWS")) {
        size_t got = fread(buf + 8, 1, file_len - 8, f);
        fclose(f);
        if (got != file_len - 8) { free(buf); return -5; }
    } else if (!strcmp(sig, "CWS")) {
        z_stream zs;
        memset(&zs, 0, sizeof(zs));
        if (inflateInit(&zs) != Z_OK) { fclose(f); free(buf); return -6; }

        uint8_t inbuf[2048];
        zs.next_out = buf + 8;
        zs.avail_out = (uInt)(file_len - 8);

        int ret = Z_OK;
        while (ret != Z_STREAM_END && zs.avail_out > 0) {
            if (zs.avail_in == 0) {
                size_t n = fread(inbuf, 1, sizeof(inbuf), f);
                if (n == 0) break;
                zs.next_in = inbuf;
                zs.avail_in = (uInt)n;
            }
            ret = inflate(&zs, Z_NO_FLUSH);
            if (ret != Z_OK && ret != Z_STREAM_END) {
                inflateEnd(&zs); fclose(f); free(buf); return -7;
            }
        }

        inflateEnd(&zs);
        fclose(f);
        if (ret != Z_STREAM_END) { free(buf); return -8; }
    } else {
        // ZWS (LZMA) not supported yet
        fclose(f);
        free(buf);
        return -9;
    }

    *out_buf = buf;
    *out_len = file_len;
    return 0;
}

static const char* tag_name(uint16_t code) {
    switch (code) {
        case 0:  return "End";
        case 1:  return "ShowFrame";
        case 2:  return "DefineShape";
        case 4:  return "PlaceObject";
        case 5:  return "RemoveObject";
        case 9:  return "SetBackgroundColor";
        case 12: return "DoAction";
        case 26: return "PlaceObject2";
        case 28: return "RemoveObject2";
        case 39: return "DefineSprite";
        case 43: return "FrameLabel";
        case 45: return "SoundStreamHead2";
        case 69: return "FileAttributes";
        case 70: return "PlaceObject3";
        case 73: return "DefineFontAlignZones";
        case 74: return "CSMTextSettings";
        case 75: return "DefineFont3";
        case 76: return "SymbolClass";
        case 82: return "DoABC";
        case 83: return "DefineShape4";
        default: return "?";
    }
}

// Scan a tag stream (root timeline or sprite timeline)
static int scan_stream(const uint8_t* data, size_t len,
                       SwfTagSummary* out,
                       int print_limit, int* printed,
                       int indent, bool in_sprite)
{
    size_t pos = 0;
    uint32_t local_idx = 0;

    while (pos + 2 <= len) {
        uint16_t tcl = u16le(&data[pos]); pos += 2;

        // RECORDHEADER: upper 10 bits = tag type, lower 6 bits = length; 0x3F => long length. :contentReference[oaicite:3]{index=3}
        uint16_t code = (uint16_t)(tcl >> 6);
        uint32_t size = (uint32_t)(tcl & 0x3F);

        if (size == 0x3F) {
            if (pos + 4 > len) break;
            size = u32le(&data[pos]); pos += 4;
        }
        if (pos + size > len) break;

        local_idx++;

        if (in_sprite) {
            out->sprite_tags++;
            if (code == 1) out->sprite_showframe_tags++;
        } else {
            out->total_tags++;
            if (code == 1) out->showframe_tags++;
        }

        // FileAttributes is a root-level tag (and SWF8+ requires it very early). :contentReference[oaicite:4]{index=4}
        if (!in_sprite && code == 69 && size >= 4) {
            // These flags are stored in the first byte of a 32-bit field:
            // UseNetwork (bit0), ActionScript3 (bit3), HasMetadata (bit4), etc. :contentReference[oaicite:5]{index=5}
            uint32_t flags = u32le(&data[pos]);
            out->has_file_attributes = true;
            out->use_network  = (flags & (1u << 0)) != 0;
            out->use_as3      = (flags & (1u << 3)) != 0;
            out->has_metadata = (flags & (1u << 4)) != 0;
        }

        // Print tag line (global print limit applies across root + sprites)
        if (*printed < print_limit) {
            for (int i = 0; i < indent; i++) putchar(' ');

            if (!in_sprite) {
                printf("%4u: tag=%u (%s), len=%u\n",
                       (unsigned)out->total_tags, (unsigned)code, tag_name(code), (unsigned)size);
            } else {
                printf("  s%3u: tag=%u (%s), len=%u\n",
                       (unsigned)local_idx, (unsigned)code, tag_name(code), (unsigned)size);
            }
            (*printed)++;
        }

        // Recurse into DefineSprite (tag 39): SpriteID(UI16), FrameCount(UI16), then ControlTags until End. :contentReference[oaicite:6]{index=6}
        if (!in_sprite && code == 39 && size >= 4) {
            uint16_t sprite_id = u16le(&data[pos + 0]);
            uint16_t frames    = u16le(&data[pos + 2]);
            (void)frames;

            out->sprite_count++;

            // Optional: print one extra informative line (doesn't count as "a tag")
            if (*printed < print_limit) {
                for (int i = 0; i < indent + 2; i++) putchar(' ');
                printf("DefineSprite details: id=%u, frames=%u\n", (unsigned)sprite_id, (unsigned)frames);
            }

            // Scan the sprite's control tags
            const uint8_t* inner = data + pos + 4;
            size_t inner_len = (size_t)(size - 4);
            scan_stream(inner, inner_len, out, print_limit, printed, indent + 2, true);
        }

        pos += size;

        // End tag terminates current stream (root file OR sprite). :contentReference[oaicite:7]{index=7}
        if (code == 0) break;
    }

    return 0;
}

int swf_scan_tags(const char* path, SwfTagSummary* out, int print_first_n) {
    memset(out, 0, sizeof(*out));

    uint8_t* swf = NULL;
    size_t len = 0;
    char sig[4];
    int rc = swf_load_uncompressed(path, &swf, &len, sig);
    if (rc != 0) {
        printf("swf_load_uncompressed failed rc=%d (sig=%s)\n", rc, sig);
        if (!strcmp(sig, "ZWS")) printf("ZWS (LZMA) not supported yet.\n");
        return rc;
    }

    size_t off = 0;
    rc = swf_tag_start_offset(swf, len, &off);
    if (rc != 0) { free(swf); return -20; }

    int printed = 0;
    scan_stream(swf + off, len - off, out, print_first_n, &printed, 0, false);

    free(swf);
    return 0;
}
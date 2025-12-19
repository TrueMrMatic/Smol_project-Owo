#include "swf_parse.h"
#include <stdio.h>
#include <string.h>
#include <3ds.h>
#include <zlib.h>

static uint32_t read_u32_le(const uint8_t* p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static uint16_t read_u16_le(const uint8_t* p) {
    return (uint16_t)p[0] | ((uint16_t)p[1] << 8);
}

/* Bitreader: SWF uses big-endian bit order inside each byte. :contentReference[oaicite:3]{index=3} */
typedef struct {
    const uint8_t* buf;
    size_t len;
    size_t bitpos; // 0..len*8
} BitReader;

static uint32_t br_read_bits(BitReader* br, int n) {
    uint32_t v = 0;
    for (int i = 0; i < n; i++) {
        size_t byte_i = br->bitpos >> 3;
        int bit_i = 7 - (int)(br->bitpos & 7);
        br->bitpos++;
        uint8_t b = (byte_i < br->len) ? br->buf[byte_i] : 0;
        v = (v << 1) | ((b >> bit_i) & 1);
    }
    return v;
}
static int32_t br_read_sbits(BitReader* br, int n) {
    uint32_t u = br_read_bits(br, n);
    // sign extend
    if (n > 0 && (u & (1u << (n - 1)))) {
        u |= ~((1u << n) - 1);
    }
    return (int32_t)u;
}
static void br_byte_align(BitReader* br) {
    size_t r = br->bitpos & 7;
    if (r) br->bitpos += (8 - r);
}

/* Read enough uncompressed bytes after the first 8 bytes to parse FrameSize/Rate/Count */
static int read_uncompressed_prefix(FILE* f, const char sig[4], uint8_t* out, size_t out_cap, size_t* out_len) {
    *out_len = 0;

    if (!strcmp(sig, "FWS")) {
        // already uncompressed: just read from current file pos (right after the 8-byte header)
        *out_len = fread(out, 1, out_cap, f);
        return (*out_len > 0) ? 0 : -1;
    }

    if (!strcmp(sig, "CWS")) {
        // CWS: bytes after first 8 are zlib-compressed. :contentReference[oaicite:4]{index=4}
        z_stream zs;
        memset(&zs, 0, sizeof(zs));
        if (inflateInit(&zs) != Z_OK) return -2;

        uint8_t inbuf[1024];
        int ret = Z_OK;

        while (*out_len < out_cap) {
            if (zs.avail_in == 0) {
                size_t n = fread(inbuf, 1, sizeof(inbuf), f);
                if (n == 0) break;
                zs.next_in = inbuf;
                zs.avail_in = (uInt)n;
            }

            zs.next_out = out + *out_len;
            zs.avail_out = (uInt)(out_cap - *out_len);

            ret = inflate(&zs, Z_NO_FLUSH);

            *out_len = out_cap - zs.avail_out;

            if (ret == Z_STREAM_END) break;
            if (ret != Z_OK) { inflateEnd(&zs); return -3; }
        }

        inflateEnd(&zs);
        return (*out_len > 0) ? 0 : -4;
    }

    // ZWS (LZMA) and others not handled yet
    return -5;
}

int swf_read_header(const char* path, SwfHeader* out) {
    memset(out, 0, sizeof(*out));

    FILE* f = fopen(path, "rb");
    if (!f) return -1;

    uint8_t h8[8];
    if (fread(h8, 1, 8, f) != 8) { fclose(f); return -2; }

    out->signature[0] = (char)h8[0];
    out->signature[1] = (char)h8[1];
    out->signature[2] = (char)h8[2];
    out->signature[3] = 0;
    out->version = h8[3];
    out->file_length = read_u32_le(&h8[4]);

    // We only need a small prefix to parse FrameSize/Rate/Count
    uint8_t uc[256];
    size_t uc_len = 0;

    int r = read_uncompressed_prefix(f, out->signature, uc, sizeof(uc), &uc_len);
    fclose(f);
    if (r != 0) return -3;

    // Parse FrameSize RECT (bit-packed). Then FrameRate (UI16 8.8 fixed), FrameCount (UI16). :contentReference[oaicite:5]{index=5}
    BitReader br = { uc, uc_len, 0 };
    uint32_t nbits = br_read_bits(&br, 5);
    int32_t xmin = br_read_sbits(&br, (int)nbits);
    int32_t xmax = br_read_sbits(&br, (int)nbits);
    int32_t ymin = br_read_sbits(&br, (int)nbits);
    int32_t ymax = br_read_sbits(&br, (int)nbits);

    br_byte_align(&br);

    size_t byte_pos = br.bitpos >> 3;
    if (byte_pos + 4 > uc_len) return -4;

    uint16_t fr = read_u16_le(&uc[byte_pos + 0]);
    uint16_t fc = read_u16_le(&uc[byte_pos + 2]);

    // RECT values are in twips (1/20 px). :contentReference[oaicite:6]{index=6}
    int w_twips = (int)(xmax - xmin);
    int h_twips = (int)(ymax - ymin);
    out->width_px  = w_twips / 20;
    out->height_px = h_twips / 20;

    out->fps = (float)fr / 256.0f;
    out->frame_count = fc;

    return 0;
}
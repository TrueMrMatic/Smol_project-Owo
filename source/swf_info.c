#include "swf_info.h"
#include "swf_parse.h"
#include "swf_tags.h"

#include <stdio.h>
#include <string.h>
#include <3ds.h>

void swf_show_header(const char* fullpath)
{
    consoleClear();
    printf("SWF info (read-only)\n\n");
    printf("File: %s\n\n", fullpath);

    SwfHeader hdr;
    int rc = swf_read_header(fullpath, &hdr);

    if (rc != 0) {
        printf("ERROR: swf_read_header failed (rc=%d)\n", rc);
        printf("If the file is ZWS (LZMA) it is not supported yet.\n");
        printf("\nPress B to go back.\n");
        return;
    }

    printf("Signature: %s\n", hdr.signature);
    printf("Version:   %u\n", (unsigned)hdr.version);
    printf("Declared size (decompressed): %u bytes\n\n", (unsigned)hdr.file_length);

    if (!strcmp(hdr.signature, "FWS")) {
        printf("Type: Uncompressed SWF\n");
    } else if (!strcmp(hdr.signature, "CWS")) {
        printf("Type: Zlib-compressed SWF\n");
    } else if (!strcmp(hdr.signature, "ZWS")) {
        printf("Type: LZMA-compressed SWF (not supported yet)\n");
    } else {
        printf("Type: Unknown\n");
    }

    // These come from FrameSize/FrameRate/FrameCount in the SWF header.
    if (!strcmp(hdr.signature, "FWS") || !strcmp(hdr.signature, "CWS")) {
        printf("\nStage: %d x %d px\n", hdr.width_px, hdr.height_px);
        printf("FPS:   %.2f\n", hdr.fps);
        printf("Frames:%u\n", (unsigned)hdr.frame_count);
    }
	
	if (!strcmp(hdr.signature, "FWS") || !strcmp(hdr.signature, "CWS")) {
		printf("\n--- Tag scan (first 30) ---\n");
		SwfTagSummary s;
		rc = swf_scan_tags(fullpath, &s, 15);
		if (rc == 0) {
			printf("\nTotal tags: %u\n", (unsigned)s.total_tags);
			printf("ShowFrame tags: %u\n", (unsigned)s.showframe_tags);
			printf("Sprites: %u\n", (unsigned)s.sprite_count);
			printf("Sprite tags: %u\n", (unsigned)s.sprite_tags);
			printf("Sprite ShowFrame tags: %u\n", (unsigned)s.sprite_showframe_tags);
			if (s.has_file_attributes) {
				printf("FileAttributes: useAs3=%s, useNetwork=%s, hasMetadata=%s\n",
					   s.use_as3 ? "YES(AS3/AVM2)" : "NO(AS1/2/AVM1)",
					   s.use_network ? "YES" : "NO",
					   s.has_metadata ? "YES" : "NO");
			} else {
				printf("FileAttributes: (not found)\n");
			}
		} else {
			printf("Tag scan failed (rc=%d)\n", rc);
		}
	} else {
		printf("\nTag scan skipped (unsupported compression).\n");
	}
	
	


    printf("\nPress B to go back.\n");
}
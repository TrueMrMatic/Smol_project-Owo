#include <3ds.h>
#include <stdio.h>
#include <string.h>
#include <dirent.h>
#include <stdlib.h>
#include <strings.h>
#include "sd_browser.h"
#include "swf_info.h"
u32 __stacksize__ = 256 * 1024; // 256 KiB

#define BASE_DIR   "sdmc:/flash"
#define MAX_FILES  256
#define NAME_MAX_L 256
#define PATH_MAX_L 512

static void draw_menu(char names[MAX_FILES][NAME_MAX_L], int count, int sel, int top) {
    consoleClear();
    printf("SWF Picker (READ-ONLY)\n");
    printf("Folder: %s\n\n", BASE_DIR);

    if (count < 0) {
        printf("ERROR: Can't open folder.\n");
        printf("Create: %s\n", BASE_DIR);
        printf("and put .swf files inside.\n");
        return;
    }

    if (count == 0) {
        printf("No .swf files found.\n");
        printf("Put files in: %s\n", BASE_DIR);
        return;
    }

    printf("D-Pad Up/Down: select | A: open | START: exit\n\n");

    const int LINES = 18;
    for (int i = 0; i < LINES; i++) {
        int idx = top + i;
        if (idx >= count) break;
        printf("%c %s\n", (idx == sel) ? '>' : ' ', names[idx]);
    }

    printf("\n(%d/%d)\n", sel + 1, count);
}

int main(int argc, char* argv[]) {
    gfxInitDefault();
    consoleInit(GFX_TOP, NULL);

    static char names[MAX_FILES][NAME_MAX_L];
    int count = sd_list_swfs(BASE_DIR, names, MAX_FILES);

    int sel = 0;
    int top = 0;
    int in_viewer = 0;

    draw_menu(names, count, sel, top);

    while (aptMainLoop()) {
        hidScanInput();
        u32 kDown = hidKeysDown();

        if (kDown & KEY_START) break;

        if (!in_viewer) {
            if (count > 0) {
                if (kDown & KEY_DOWN) {
                    if (sel < count - 1) sel++;
                }
                if (kDown & KEY_UP) {
                    if (sel > 0) sel--;
                }

                // Scroll window
                const int LINES = 18;
                if (sel < top) top = sel;
                if (sel >= top + LINES) top = sel - LINES + 1;

                if (kDown & KEY_A) {
                    char path[PATH_MAX_L];
                    snprintf(path, sizeof(path), "%s/%s", BASE_DIR, names[sel]);
                    swf_show_header(path);
                    in_viewer = 1;
                    continue;
                }
            }

            if (kDown) draw_menu(names, count, sel, top);
        }
        else {
            if (kDown & KEY_B) {
                in_viewer = 0;
                draw_menu(names, count, sel, top);
            }
        }

        gfxFlushBuffers();
        gfxSwapBuffers();
        gspWaitForVBlank();
    }

    gfxExit();
    return 0;
}

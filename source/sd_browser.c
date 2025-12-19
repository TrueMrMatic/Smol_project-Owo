#include "sd_browser.h"
#include <dirent.h>
#include <string.h>
#include <stdlib.h>

static int ends_with_ignorecase(const char* s, const char* suffix) {
    size_t n = strlen(s), m = strlen(suffix);
    if (n < m) return 0;
    const char* a = s + (n - m);
    for (size_t i = 0; i < m; i++) {
        char ca = a[i], cb = suffix[i];
        if (ca >= 'A' && ca <= 'Z') ca = (char)(ca - 'A' + 'a');
        if (cb >= 'A' && cb <= 'Z') cb = (char)(cb - 'A' + 'a');
        if (ca != cb) return 0;
    }
    return 1;
}

static int cmp_str_256(const void* a, const void* b) {
    const char* sa = (const char*)a;
    const char* sb = (const char*)b;
    return strcasecmp(sa, sb);
}

int sd_list_swfs(const char* dir, char names[][SD_NAME_MAX], int max_files) {
    DIR* d = opendir(dir);
    if (!d) return -1;

    int count = 0;
    struct dirent* ent;

    while ((ent = readdir(d)) != NULL && count < max_files) {
        if (!strcmp(ent->d_name, ".") || !strcmp(ent->d_name, ".."))
            continue;

        if (ends_with_ignorecase(ent->d_name, ".swf")) {
            strncpy(names[count], ent->d_name, SD_NAME_MAX - 1);
            names[count][SD_NAME_MAX - 1] = 0;
            count++;
        }
    }
    closedir(d);

    qsort(names, (size_t)count, SD_NAME_MAX, cmp_str_256);
    return count;
}
#pragma once

#define SD_NAME_MAX 256

int sd_list_swfs(const char* dir, char names[][SD_NAME_MAX], int max_files);
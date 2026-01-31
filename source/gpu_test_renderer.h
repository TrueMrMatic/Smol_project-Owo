#ifndef GPU_TEST_RENDERER_H
#define GPU_TEST_RENDERER_H

#ifdef __cplusplus
extern "C" {
#endif

void gpu_test_renderer_init(void);
void gpu_test_renderer_draw(float t);
void gpu_test_renderer_shutdown(void);

#ifdef __cplusplus
}
#endif

#endif

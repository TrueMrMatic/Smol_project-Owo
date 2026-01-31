#include <3ds.h>
#include <citro3d.h>
#include <math.h>

#include "gpu_test_renderer.h"
#include "gpu_test_shader_shbin.h"

typedef struct {
    float x;
    float y;
    float z;
    u8 r;
    u8 g;
    u8 b;
    u8 a;
} Vertex;

static DVLB_s* g_vshader_dvlb = NULL;
static shaderProgram_s g_program;
static int g_u_loc_projection = -1;
static C3D_RenderTarget* g_target = NULL;

static const Vertex k_base_vertices[3] = {
    { -0.5f, -0.4f, 0.0f, 255,  80,  80, 255 },
    {  0.5f, -0.4f, 0.0f,  80, 255,  80, 255 },
    {  0.0f,  0.6f, 0.0f,  80,  80, 255, 255 },
};

static void setup_shader(void) {
    g_vshader_dvlb = DVLB_ParseFile((u32*)gpu_test_shader_shbin, gpu_test_shader_shbin_size);
    shaderProgramInit(&g_program);
    shaderProgramSetVsh(&g_program, &g_vshader_dvlb->DVLE[0]);
    g_u_loc_projection = shaderInstanceGetUniformLocation(g_program.vertexShader, "projection");

    C3D_BindProgram(&g_program);

    C3D_AttrInfo* attr_info = C3D_GetAttrInfo();
    AttrInfo_Init(attr_info);
    AttrInfo_AddLoader(attr_info, 0, GPU_FLOAT, 3);
    AttrInfo_AddLoader(attr_info, 1, GPU_UNSIGNED_BYTE, 4);

    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    C3D_TexEnvSrc(env, C3D_Both, GPU_PRIMARY_COLOR, 0, 0);
    C3D_TexEnvFunc(env, C3D_Both, GPU_REPLACE);

    C3D_CullFace(GPU_CULL_NONE);
}

static void setup_target(void) {
    g_target = C3D_RenderTargetCreate(240, 400, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
    C3D_RenderTargetSetOutput(
        g_target,
        GFX_TOP,
        GFX_LEFT,
        GX_TRANSFER_FLIP_VERT(0) |
        GX_TRANSFER_OUT_TILED(0) |
        GX_TRANSFER_OUT_FORMAT(GX_TRANSFER_FMT_RGB8) |
        GX_TRANSFER_IN_FORMAT(GX_TRANSFER_FMT_RGBA8) |
        GX_TRANSFER_SCALING(GX_TRANSFER_SCALE_NO)
    );
}

void gpu_test_renderer_init(void) {
    setup_shader();
    setup_target();
}

void gpu_test_renderer_draw(float t) {
    if (!g_target) {
        return;
    }

    float wobble = 0.08f * sinf(t);
    Vertex verts[3];
    for (int i = 0; i < 3; i++) {
        verts[i] = k_base_vertices[i];
        verts[i].x += wobble;
    }

    C3D_FrameBegin(C3D_FRAME_SYNCDRAW);
    C3D_FrameDrawOn(g_target);
    C3D_RenderTargetClear(g_target, C3D_CLEAR_ALL, 0x202020FF, 0);

    C3D_BindProgram(&g_program);
    C3D_Mtx projection;
    Mtx_Identity(&projection);
    if (g_u_loc_projection >= 0) {
        C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, g_u_loc_projection, &projection);
    }

    C3D_BufInfo* buf_info = C3D_GetBufInfo();
    BufInfo_Init(buf_info);
    BufInfo_Add(buf_info, verts, sizeof(Vertex), 2, 0x10);

    C3D_DrawArrays(GPU_TRIANGLES, 0, 3);
    C3D_FrameEnd(0);
}

void gpu_test_renderer_shutdown(void) {
    if (g_target) {
        C3D_RenderTargetDelete(g_target);
        g_target = NULL;
    }

    shaderProgramFree(&g_program);
    if (g_vshader_dvlb) {
        DVLB_Free(g_vshader_dvlb);
        g_vshader_dvlb = NULL;
    }
}

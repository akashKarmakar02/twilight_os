#include <twigl.h>

int main(void) {
    if (InitGlx() != 0) {
        return 1;
    }

    SetTargetFPS(60);

    while (!GlxShouldClose()) {
        BeginDrawing();
        ClearBackground(GLX_RGB(18, 24, 38));
        DrawRectangle(32, 32, GetWidth() / 4, GetHeight() / 4, GLX_RGB(0, 121, 241));
        if (IsKeyDown(KEY_A)) {
            DrawRectangle(GetWidth() / 2 - 40, GetHeight() / 2 - 40, 80, 80,
                          GLX_RGB(230, 41, 55));
        }
        EndDrawing();
    }

    CloseGlx();
    return 0;
}

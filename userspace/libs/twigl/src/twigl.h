#ifndef TWIGL_H
#define TWIGL_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t GlxColor;

#ifndef KEY_ESC
#define KEY_ESC 1
#endif
#ifndef KEY_1
#define KEY_1 2
#endif
#ifndef KEY_2
#define KEY_2 3
#endif
#ifndef KEY_3
#define KEY_3 4
#endif
#ifndef KEY_4
#define KEY_4 5
#endif
#ifndef KEY_5
#define KEY_5 6
#endif
#ifndef KEY_6
#define KEY_6 7
#endif
#ifndef KEY_7
#define KEY_7 8
#endif
#ifndef KEY_8
#define KEY_8 9
#endif
#ifndef KEY_9
#define KEY_9 10
#endif
#ifndef KEY_0
#define KEY_0 11
#endif
#ifndef KEY_A
#define KEY_A 30
#endif
#ifndef KEY_B
#define KEY_B 48
#endif
#ifndef KEY_C
#define KEY_C 46
#endif
#ifndef KEY_D
#define KEY_D 32
#endif
#ifndef KEY_E
#define KEY_E 18
#endif
#ifndef KEY_F
#define KEY_F 33
#endif
#ifndef KEY_G
#define KEY_G 34
#endif
#ifndef KEY_H
#define KEY_H 35
#endif
#ifndef KEY_I
#define KEY_I 23
#endif
#ifndef KEY_J
#define KEY_J 36
#endif
#ifndef KEY_K
#define KEY_K 37
#endif
#ifndef KEY_L
#define KEY_L 38
#endif
#ifndef KEY_M
#define KEY_M 50
#endif
#ifndef KEY_N
#define KEY_N 49
#endif
#ifndef KEY_O
#define KEY_O 24
#endif
#ifndef KEY_P
#define KEY_P 25
#endif
#ifndef KEY_Q
#define KEY_Q 16
#endif
#ifndef KEY_R
#define KEY_R 19
#endif
#ifndef KEY_S
#define KEY_S 31
#endif
#ifndef KEY_T
#define KEY_T 20
#endif
#ifndef KEY_U
#define KEY_U 22
#endif
#ifndef KEY_V
#define KEY_V 47
#endif
#ifndef KEY_W
#define KEY_W 17
#endif
#ifndef KEY_X
#define KEY_X 45
#endif
#ifndef KEY_Y
#define KEY_Y 21
#endif
#ifndef KEY_Z
#define KEY_Z 44
#endif
#ifndef KEY_SPACE
#define KEY_SPACE 57
#endif
#ifndef KEY_ENTER
#define KEY_ENTER 28
#endif
#ifndef KEY_TAB
#define KEY_TAB 15
#endif
#ifndef KEY_BACKSPACE
#define KEY_BACKSPACE 14
#endif
#ifndef KEY_LEFT
#define KEY_LEFT 105
#endif
#ifndef KEY_RIGHT
#define KEY_RIGHT 106
#endif
#ifndef KEY_UP
#define KEY_UP 103
#endif
#ifndef KEY_DOWN
#define KEY_DOWN 108
#endif

#define GLX_RGBA(r, g, b, a) \
    ((((uint32_t)(a) & 0xffu) << 24) | (((uint32_t)(r) & 0xffu) << 16) | \
     (((uint32_t)(g) & 0xffu) << 8) | ((uint32_t)(b) & 0xffu))
#define GLX_RGB(r, g, b) GLX_RGBA((r), (g), (b), 255)

#ifndef BLACK
#define BLACK GLX_RGB(0, 0, 0)
#endif
#ifndef WHITE
#define WHITE GLX_RGB(255, 255, 255)
#endif
#ifndef RED
#define RED GLX_RGB(230, 41, 55)
#endif
#ifndef GREEN
#define GREEN GLX_RGB(0, 228, 48)
#endif
#ifndef BLUE
#define BLUE GLX_RGB(0, 121, 241)
#endif

int InitGlx(void);
void CloseGlx(void);
int GlxShouldClose(void);

void BeginDrawing(void);
void EndDrawing(void);
void ClearBackground(GlxColor color);
void DrawRectangle(int x, int y, int width, int height, GlxColor color);
void SetTargetFPS(int fps);
int IsKeyPressed(int key);
int IsKeyDown(int key);

int GetWidth(void);
int GetHeight(void);
int GetScreenWidth(void);
int GetScreenHeight(void);
float GetFrameTime(void);
double GetTime(void);

#ifdef __cplusplus
}
#endif

#endif

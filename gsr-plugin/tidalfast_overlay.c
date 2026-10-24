/* GPU Screen Recorder plugin: draws tidalfast's now-playing card into the
 * recording. The card is rendered by tidalfast (Settings -> Overlay) and
 * fetched from its local web server as premultiplied RGBA pixels; this plugin
 * only uploads it as a texture and draws one quad in a corner.
 *
 * Optional settings in ~/.config/tidalfast/gsr-overlay.conf, one per line:
 *   port=7788   corner=bl|br|tl|tr   scale=1.0   margin=24   flip=0|1
 */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <dlfcn.h>
#include <gsr/plugin.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

/* ---- the few OpenGL (ES) entry points we need, resolved at run time ---- */
typedef unsigned int GLuint;
typedef int GLint;
typedef int GLsizei;
typedef unsigned int GLenum;
typedef float GLfloat;
typedef char GLchar;
typedef unsigned char GLboolean;

#define GL_TEXTURE_2D 0x0DE1
#define GL_TEXTURE0 0x84C0
#define GL_RGBA 0x1908
#define GL_UNSIGNED_BYTE 0x1401
#define GL_FLOAT 0x1406
#define GL_TRIANGLE_STRIP 0x0005
#define GL_BLEND 0x0BE2
#define GL_DEPTH_TEST 0x0B71
#define GL_CULL_FACE 0x0B44
#define GL_SCISSOR_TEST 0x0C11
#define GL_ONE 1
#define GL_ONE_MINUS_SRC_ALPHA 0x0303
#define GL_ARRAY_BUFFER 0x8892
#define GL_STATIC_DRAW 0x88E4
#define GL_DYNAMIC_DRAW 0x88E8
#define GL_VERTEX_SHADER 0x8B31
#define GL_FRAGMENT_SHADER 0x8B30
#define GL_COMPILE_STATUS 0x8B81
#define GL_LINK_STATUS 0x8B82
#define GL_TEXTURE_MIN_FILTER 0x2801
#define GL_TEXTURE_MAG_FILTER 0x2800
#define GL_TEXTURE_WRAP_S 0x2802
#define GL_TEXTURE_WRAP_T 0x2803
#define GL_LINEAR 0x2601
#define GL_CLAMP_TO_EDGE 0x812F
#define GL_UNPACK_ALIGNMENT 0x0CF5
#define GL_CURRENT_PROGRAM 0x8B8D
#define GL_ARRAY_BUFFER_BINDING 0x8894
#define GL_TEXTURE_BINDING_2D 0x8069
#define GL_ACTIVE_TEXTURE 0x84E0
#define GL_VERTEX_ARRAY_BINDING 0x85B5
#define GL_VIEWPORT 0x0BA2
#define GL_BLEND_SRC_RGB 0x80C9
#define GL_BLEND_DST_RGB 0x80C8
#define GL_BLEND_SRC_ALPHA 0x80CB
#define GL_BLEND_DST_ALPHA 0x80CA

#define FN(ret, name, args) static ret(*name) args
FN(void, p_glViewport, (GLint, GLint, GLsizei, GLsizei));
FN(void, p_glEnable, (GLenum));
FN(void, p_glDisable, (GLenum));
FN(GLboolean, p_glIsEnabled, (GLenum));
FN(void, p_glBlendFuncSeparate, (GLenum, GLenum, GLenum, GLenum));
FN(void, p_glGetIntegerv, (GLenum, GLint *));
FN(void, p_glActiveTexture, (GLenum));
FN(void, p_glBindTexture, (GLenum, GLuint));
FN(void, p_glGenTextures, (GLsizei, GLuint *));
FN(void, p_glDeleteTextures, (GLsizei, const GLuint *));
FN(void, p_glTexParameteri, (GLenum, GLenum, GLint));
FN(void, p_glTexImage2D, (GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, const void *));
FN(void, p_glTexSubImage2D, (GLenum, GLint, GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, const void *));
FN(void, p_glPixelStorei, (GLenum, GLint));
FN(GLuint, p_glCreateShader, (GLenum));
FN(void, p_glShaderSource, (GLuint, GLsizei, const GLchar *const *, const GLint *));
FN(void, p_glCompileShader, (GLuint));
FN(void, p_glGetShaderiv, (GLuint, GLenum, GLint *));
FN(void, p_glDeleteShader, (GLuint));
FN(GLuint, p_glCreateProgram, (void));
FN(void, p_glAttachShader, (GLuint, GLuint));
FN(void, p_glLinkProgram, (GLuint));
FN(void, p_glGetProgramiv, (GLuint, GLenum, GLint *));
FN(void, p_glUseProgram, (GLuint));
FN(void, p_glDeleteProgram, (GLuint));
FN(GLint, p_glGetAttribLocation, (GLuint, const GLchar *));
FN(GLint, p_glGetUniformLocation, (GLuint, const GLchar *));
FN(void, p_glUniform1i, (GLint, GLint));
FN(void, p_glGenBuffers, (GLsizei, GLuint *));
FN(void, p_glDeleteBuffers, (GLsizei, const GLuint *));
FN(void, p_glBindBuffer, (GLenum, GLuint));
FN(void, p_glBufferData, (GLenum, long, const void *, GLenum));
FN(void, p_glEnableVertexAttribArray, (GLuint));
FN(void, p_glDisableVertexAttribArray, (GLuint));
FN(void, p_glVertexAttribPointer, (GLuint, GLint, GLenum, GLboolean, GLsizei, const void *));
FN(void, p_glDrawArrays, (GLenum, GLint, GLsizei));
FN(void, p_glGenVertexArrays, (GLsizei, GLuint *));
FN(void, p_glBindVertexArray, (GLuint));
FN(void, p_glDeleteVertexArrays, (GLsizei, const GLuint *));

static gsr_plugin_graphics_api g_api;
static void *(*get_proc)(const char *);
static void *libs[4];
static int nlibs;

/* The recorder loads its GL libraries privately, so their symbols are not
 * visible globally: look in the libraries it already has loaded. */
static void *open_loaded(const char *name) {
    void *h = dlopen(name, RTLD_NOW | RTLD_NOLOAD);
    if (h && nlibs < 4) libs[nlibs++] = h;
    return h;
}

static void *resolve(const char *name) {
    void *f = dlsym(RTLD_DEFAULT, name);
    for (int i = 0; !f && i < nlibs; i++) f = dlsym(libs[i], name);
    if (!f && get_proc) f = get_proc(name);
    return f;
}

#define LOAD(name) (p_##name = resolve(#name))
static int load_gl(void) {
    nlibs = 0;
    void *egl = open_loaded("libEGL.so.1");
    void *glx = open_loaded("libGLX.so.0");
    open_loaded("libGL.so.1");
    open_loaded("libGLESv2.so.2");
    if (g_api == GSR_PLUGIN_GRAPHICS_API_EGL_ES) {
        if (egl) get_proc = dlsym(egl, "eglGetProcAddress");
    } else if (glx) {
        get_proc = dlsym(glx, "glXGetProcAddressARB");
        if (!get_proc) get_proc = dlsym(glx, "glXGetProcAddress");
    }
    if (!get_proc) get_proc = dlsym(RTLD_DEFAULT, "eglGetProcAddress");
    if (!get_proc) get_proc = dlsym(RTLD_DEFAULT, "glXGetProcAddress");
    int ok = 1;
    ok &= !!LOAD(glViewport); ok &= !!LOAD(glEnable); ok &= !!LOAD(glDisable); ok &= !!LOAD(glIsEnabled);
    ok &= !!LOAD(glBlendFuncSeparate); ok &= !!LOAD(glGetIntegerv); ok &= !!LOAD(glActiveTexture);
    ok &= !!LOAD(glBindTexture); ok &= !!LOAD(glGenTextures); ok &= !!LOAD(glDeleteTextures);
    ok &= !!LOAD(glTexParameteri); ok &= !!LOAD(glTexImage2D); ok &= !!LOAD(glTexSubImage2D);
    ok &= !!LOAD(glPixelStorei); ok &= !!LOAD(glCreateShader); ok &= !!LOAD(glShaderSource);
    ok &= !!LOAD(glCompileShader); ok &= !!LOAD(glGetShaderiv); ok &= !!LOAD(glDeleteShader);
    ok &= !!LOAD(glCreateProgram); ok &= !!LOAD(glAttachShader); ok &= !!LOAD(glLinkProgram);
    ok &= !!LOAD(glGetProgramiv); ok &= !!LOAD(glUseProgram); ok &= !!LOAD(glDeleteProgram);
    ok &= !!LOAD(glGetAttribLocation); ok &= !!LOAD(glGetUniformLocation); ok &= !!LOAD(glUniform1i);
    ok &= !!LOAD(glGenBuffers); ok &= !!LOAD(glDeleteBuffers); ok &= !!LOAD(glBindBuffer);
    ok &= !!LOAD(glBufferData); ok &= !!LOAD(glEnableVertexAttribArray);
    ok &= !!LOAD(glDisableVertexAttribArray); ok &= !!LOAD(glVertexAttribPointer); ok &= !!LOAD(glDrawArrays);
    LOAD(glGenVertexArrays); LOAD(glBindVertexArray); LOAD(glDeleteVertexArrays); /* optional */
    return ok;
}

/* ---- settings ---- */
typedef struct {
    int port;
    char corner[3];
    float scale;
    int margin;
    int flip;
} Config;

static void load_config(Config *c) {
    c->port = 7788; strcpy(c->corner, "bl"); c->scale = 1.0f; c->margin = 24; c->flip = 0;
    const char *base = getenv("XDG_CONFIG_HOME");
    char path[1024];
    if (base && *base) snprintf(path, sizeof path, "%s/tidalfast/gsr-overlay.conf", base);
    else snprintf(path, sizeof path, "%s/.config/tidalfast/gsr-overlay.conf", getenv("HOME") ? getenv("HOME") : "");
    FILE *f = fopen(path, "r");
    if (!f) return;
    char line[256];
    while (fgets(line, sizeof line, f)) {
        char *eq = strchr(line, '=');
        if (!eq) continue;
        *eq = 0;
        const char *k = line, *v = eq + 1;
        if (!strcmp(k, "port")) c->port = atoi(v);
        else if (!strcmp(k, "corner") && strlen(v) >= 2) { c->corner[0] = v[0]; c->corner[1] = v[1]; c->corner[2] = 0; }
        else if (!strcmp(k, "scale")) c->scale = (float)atof(v);
        else if (!strcmp(k, "margin")) c->margin = atoi(v);
        else if (!strcmp(k, "flip")) c->flip = atoi(v);
    }
    fclose(f);
    if (c->port < 1 || c->port > 65535) c->port = 7788;
    if (c->scale < 0.3f || c->scale > 4.0f) c->scale = 1.0f;
}

/* ---- state shared between the fetching thread and draw() ---- */
typedef struct {
    Config cfg;
    unsigned int video_w, video_h;
    float card_scale;

    pthread_t thread;
    int thread_started;
    volatile int stop;

    pthread_mutex_t lock;
    uint8_t *pixels; /* latest card, or NULL when nothing plays */
    unsigned int card_w, card_h;
    int fresh;  /* pixels changed since the last upload */
    int damage; /* something visible changed since clear_damage */

    /* GL objects, created on the first draw */
    int gl_ready, gl_failed;
    GLuint prog, tex, vbo, vao;
    GLint loc_pos, loc_uv, loc_tex;
    unsigned int tex_w, tex_h;
    int visible;
} Plugin;

static int fetch(Plugin *p, uint8_t **out, unsigned int *w, unsigned int *h) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return 0;
    struct timeval tv = {3, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_port = htons((uint16_t)p->cfg.port);
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (connect(fd, (struct sockaddr *)&a, sizeof a) < 0) { close(fd); return 0; }
    char req[160];
    int n = snprintf(req, sizeof req, "GET /card.rgba?scale=%.3f HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", p->card_scale);
    if (write(fd, req, (size_t)n) != n) { close(fd); return 0; }

    size_t cap = 1 << 16, len = 0;
    char *buf = malloc(cap);
    for (;;) {
        if (len == cap) { cap *= 2; buf = realloc(buf, cap); }
        ssize_t r = read(fd, buf + len, cap - len);
        if (r <= 0) break;
        len += (size_t)r;
    }
    close(fd);
    int ok = 0;
    char *body = NULL;
    if (len > 12 && !memcmp(buf, "HTTP/1.1 200", 12) && (body = memmem(buf, len, "\r\n\r\n", 4))) {
        body += 4;
        unsigned int cw = 0, ch = 0;
        char *hw = memmem(buf, len, "X-Width: ", 9), *hh = memmem(buf, len, "X-Height: ", 10);
        if (hw && hh) { cw = (unsigned)atoi(hw + 9); ch = (unsigned)atoi(hh + 10); }
        size_t want = (size_t)cw * ch * 4;
        if (cw && ch && cw <= 4096 && ch <= 2048 && (size_t)(buf + len - body) >= want) {
            *out = malloc(want);
            memcpy(*out, body, want);
            *w = cw; *h = ch;
            ok = 1;
        }
    }
    free(buf);
    return ok;
}

static void *poll_thread(void *arg) {
    Plugin *p = arg;
    while (!p->stop) {
        uint8_t *px = NULL;
        unsigned int w = 0, h = 0;
        int ok = fetch(p, &px, &w, &h);
        pthread_mutex_lock(&p->lock);
        if (ok) {
            free(p->pixels);
            p->pixels = px; p->card_w = w; p->card_h = h;
            p->fresh = 1; p->damage = 1;
        } else if (p->pixels) {
            free(p->pixels);
            p->pixels = NULL;
            p->damage = 1;
        }
        pthread_mutex_unlock(&p->lock);
        for (int i = 0; i < 10 && !p->stop; i++) {
            struct timespec t = {0, 100 * 1000 * 1000};
            nanosleep(&t, NULL);
        }
    }
    return NULL;
}

/* ---- drawing ---- */
static GLuint compile(GLenum kind, const char *src) {
    GLuint s = p_glCreateShader(kind);
    p_glShaderSource(s, 1, &src, NULL);
    p_glCompileShader(s);
    GLint ok = 0;
    p_glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    if (!ok) { p_glDeleteShader(s); return 0; }
    return s;
}

static int make_program(Plugin *p, const char *vs, const char *fs) {
    GLuint v = compile(GL_VERTEX_SHADER, vs), f = compile(GL_FRAGMENT_SHADER, fs);
    if (!v || !f) return 0;
    p->prog = p_glCreateProgram();
    p_glAttachShader(p->prog, v);
    p_glAttachShader(p->prog, f);
    p_glLinkProgram(p->prog);
    GLint ok = 0;
    p_glGetProgramiv(p->prog, GL_LINK_STATUS, &ok);
    p_glDeleteShader(v);
    p_glDeleteShader(f);
    if (!ok) { p_glDeleteProgram(p->prog); p->prog = 0; return 0; }
    return 1;
}

static void setup_gl(Plugin *p, gsr_plugin_graphics_api api) {
    if (!load_gl()) { p->gl_failed = 1; fprintf(stderr, "tidalfast overlay: OpenGL functions missing\n"); return; }
    fprintf(stderr, "tidalfast overlay: drawing with %s\n", g_api == GSR_PLUGIN_GRAPHICS_API_EGL_ES ? "EGL/GLES" : "GLX");
    static const char *vs_es = "#version 100\nattribute vec2 pos; attribute vec2 uv; varying vec2 v;\n"
        "void main(){ v = uv; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    static const char *fs_es = "#version 100\nprecision mediump float; varying vec2 v; uniform sampler2D tex;\n"
        "void main(){ gl_FragColor = texture2D(tex, v); }\n";
    static const char *vs_gl = "#version 120\nattribute vec2 pos; attribute vec2 uv; varying vec2 v;\n"
        "void main(){ v = uv; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    static const char *fs_gl = "#version 120\nvarying vec2 v; uniform sampler2D tex;\n"
        "void main(){ gl_FragColor = texture2D(tex, v); }\n";
    int ok = api == GSR_PLUGIN_GRAPHICS_API_EGL_ES ? make_program(p, vs_es, fs_es) : make_program(p, vs_gl, fs_gl);
    if (!ok) ok = make_program(p, vs_es, fs_es);
    if (!ok) ok = make_program(p, vs_gl, fs_gl);
    if (!ok) { p->gl_failed = 1; fprintf(stderr, "tidalfast overlay: shader failed\n"); return; }
    p->loc_pos = p_glGetAttribLocation(p->prog, "pos");
    p->loc_uv = p_glGetAttribLocation(p->prog, "uv");
    p->loc_tex = p_glGetUniformLocation(p->prog, "tex");
    p_glGenTextures(1, &p->tex);
    p_glBindTexture(GL_TEXTURE_2D, p->tex);
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    p_glGenBuffers(1, &p->vbo);
    p->gl_ready = 1;
}

static void draw(const gsr_plugin_draw_params *params, void *userdata) {
    Plugin *p = userdata;
    if (p->gl_failed) return;

    pthread_mutex_lock(&p->lock);
    int have = p->pixels != NULL;
    if (!have) { pthread_mutex_unlock(&p->lock); return; }
    pthread_mutex_unlock(&p->lock);

    /* remember what we are about to change */
    GLint old_prog, old_buf, old_tex, old_active, old_vao = 0, old_vp[4], bs_rgb, bd_rgb, bs_a, bd_a;
    GLboolean was_blend = 0, was_depth = 0, was_cull = 0, was_scissor = 0;
    if (!p->gl_ready) setup_gl(p, g_api);
    if (p->gl_failed) return;
    p_glGetIntegerv(GL_CURRENT_PROGRAM, &old_prog);
    p_glGetIntegerv(GL_ARRAY_BUFFER_BINDING, &old_buf);
    p_glGetIntegerv(GL_ACTIVE_TEXTURE, &old_active);
    p_glActiveTexture(GL_TEXTURE0);
    p_glGetIntegerv(GL_TEXTURE_BINDING_2D, &old_tex);
    p_glGetIntegerv(GL_VIEWPORT, old_vp);
    p_glGetIntegerv(GL_BLEND_SRC_RGB, &bs_rgb); p_glGetIntegerv(GL_BLEND_DST_RGB, &bd_rgb);
    p_glGetIntegerv(GL_BLEND_SRC_ALPHA, &bs_a); p_glGetIntegerv(GL_BLEND_DST_ALPHA, &bd_a);
    was_blend = p_glIsEnabled(GL_BLEND); was_depth = p_glIsEnabled(GL_DEPTH_TEST);
    was_cull = p_glIsEnabled(GL_CULL_FACE); was_scissor = p_glIsEnabled(GL_SCISSOR_TEST);
    if (p_glGenVertexArrays && p_glBindVertexArray) {
        p_glGetIntegerv(GL_VERTEX_ARRAY_BINDING, &old_vao);
        if (!p->vao) p_glGenVertexArrays(1, &p->vao);
        p_glBindVertexArray(p->vao);
    }

    pthread_mutex_lock(&p->lock);
    p_glBindTexture(GL_TEXTURE_2D, p->tex);
    if (p->fresh && p->pixels) {
        p_glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        if (p->tex_w != p->card_w || p->tex_h != p->card_h) {
            p_glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, (GLsizei)p->card_w, (GLsizei)p->card_h, 0, GL_RGBA, GL_UNSIGNED_BYTE, p->pixels);
            p->tex_w = p->card_w; p->tex_h = p->card_h;
        } else {
            p_glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, (GLsizei)p->card_w, (GLsizei)p->card_h, GL_RGBA, GL_UNSIGNED_BYTE, p->pixels);
        }
        p->fresh = 0;
    }
    float cw = (float)p->card_w, ch = (float)p->card_h;
    pthread_mutex_unlock(&p->lock);

    /* quad in pixels (origin bottom left, y up), then to clip space */
    float W = (float)params->width, H = (float)params->height, m = (float)p->cfg.margin * p->card_scale;
    float x0 = p->cfg.corner[1] == 'r' ? W - m - cw : m;
    float y0 = p->cfg.corner[0] == 't' ? H - m - ch : m;
    float x1 = x0 + cw, y1 = y0 + ch;
    /* texture row 0 is the card's top: it goes to the quad's upper edge, or the lower one when flipped */
    float vt = p->cfg.flip ? 1.0f : 0.0f, vb = p->cfg.flip ? 0.0f : 1.0f;
    #define NX(x) ((x) / W * 2.0f - 1.0f)
    #define NY(y) ((y) / H * 2.0f - 1.0f)
    float quad[16] = {
        NX(x0), NY(y0), 0.0f, vb,
        NX(x1), NY(y0), 1.0f, vb,
        NX(x0), NY(y1), 0.0f, vt,
        NX(x1), NY(y1), 1.0f, vt,
    };

    p_glViewport(0, 0, (GLsizei)params->width, (GLsizei)params->height);
    p_glDisable(GL_DEPTH_TEST); p_glDisable(GL_CULL_FACE); p_glDisable(GL_SCISSOR_TEST);
    p_glEnable(GL_BLEND);
    p_glBlendFuncSeparate(GL_ONE, GL_ONE_MINUS_SRC_ALPHA, GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
    p_glUseProgram(p->prog);
    p_glUniform1i(p->loc_tex, 0);
    p_glBindBuffer(GL_ARRAY_BUFFER, p->vbo);
    p_glBufferData(GL_ARRAY_BUFFER, sizeof quad, quad, GL_DYNAMIC_DRAW);
    p_glEnableVertexAttribArray((GLuint)p->loc_pos);
    p_glVertexAttribPointer((GLuint)p->loc_pos, 2, GL_FLOAT, 0, 4 * sizeof(float), (const void *)0);
    p_glEnableVertexAttribArray((GLuint)p->loc_uv);
    p_glVertexAttribPointer((GLuint)p->loc_uv, 2, GL_FLOAT, 0, 4 * sizeof(float), (const void *)(2 * sizeof(float)));
    p_glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    p_glDisableVertexAttribArray((GLuint)p->loc_pos);
    p_glDisableVertexAttribArray((GLuint)p->loc_uv);

    /* put everything back */
    if (p_glGenVertexArrays && p_glBindVertexArray) p_glBindVertexArray((GLuint)old_vao);
    p_glBindBuffer(GL_ARRAY_BUFFER, (GLuint)old_buf);
    p_glUseProgram((GLuint)old_prog);
    p_glBindTexture(GL_TEXTURE_2D, (GLuint)old_tex);
    p_glActiveTexture((GLenum)old_active);
    p_glBlendFuncSeparate((GLenum)bs_rgb, (GLenum)bd_rgb, (GLenum)bs_a, (GLenum)bd_a);
    if (!was_blend) p_glDisable(GL_BLEND);
    if (was_depth) p_glEnable(GL_DEPTH_TEST);
    if (was_cull) p_glEnable(GL_CULL_FACE);
    if (was_scissor) p_glEnable(GL_SCISSOR_TEST);
    p_glViewport(old_vp[0], old_vp[1], old_vp[2], old_vp[3]);
}

static bool is_damaged(void *userdata) {
    Plugin *p = userdata;
    pthread_mutex_lock(&p->lock);
    int d = p->damage;
    pthread_mutex_unlock(&p->lock);
    return d;
}

static void clear_damage(void *userdata) {
    Plugin *p = userdata;
    pthread_mutex_lock(&p->lock);
    p->damage = 0;
    pthread_mutex_unlock(&p->lock);
}

__attribute__((visibility("default"))) bool gsr_plugin_init(const gsr_plugin_init_params *params, gsr_plugin_init_return *ret) {
    Plugin *p = calloc(1, sizeof *p);
    if (!p) return false;
    load_config(&p->cfg);
    pthread_mutex_init(&p->lock, NULL);
    p->video_w = params->width; p->video_h = params->height;
    p->card_scale = p->cfg.scale * ((float)params->height / 1080.0f);
    if (p->card_scale < 0.5f) p->card_scale = 0.5f;
    if (p->card_scale > 3.0f) p->card_scale = 3.0f;
    g_api = params->graphics_api;
    if (pthread_create(&p->thread, NULL, poll_thread, p) == 0) p->thread_started = 1;

    ret->name = "tidalfast now playing";
    ret->version = 1;
    ret->userdata = p;
    ret->draw = draw;
    ret->is_damaged = is_damaged;
    ret->clear_damage = clear_damage;
    return true;
}

__attribute__((visibility("default"))) void gsr_plugin_deinit(void *userdata) {
    Plugin *p = userdata;
    if (!p) return;
    p->stop = 1;
    if (p->thread_started) pthread_join(p->thread, NULL);
    free(p->pixels);
    pthread_mutex_destroy(&p->lock);
    /* GL objects are released with the recorder's context */
    free(p);
}

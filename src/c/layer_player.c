#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <unistd.h>
#include <poll.h>
#include <signal.h>
#include <wayland-client.h>
#include <wayland-egl.h>
#include <EGL/egl.h>
#include <GL/gl.h>
#include <mpv/client.h>
#include <mpv/render_gl.h>

#include "wlr-layer-shell.h"

static volatile sig_atomic_t running = 1;

static void sig_handler(int sig) {
    (void)sig;
    running = 0;
}

struct app_state {
    struct wl_display *display;
    struct wl_registry *registry;
    struct wl_compositor *compositor;
    struct zwlr_layer_shell_v1 *layer_shell;
    struct wl_output *output;
    char target_output_name[64];

    struct wl_surface *surface;
    struct zwlr_layer_surface_v1 *layer_surface;
    struct wl_egl_window *egl_window;

    EGLDisplay egl_display;
    EGLContext egl_context;
    EGLSurface egl_surface;
    EGLConfig egl_config;

    mpv_handle *mpv;
    mpv_render_context *mpv_gl;

    int32_t width;
    int32_t height;
    bool configured;
    bool need_render;
};

static void *get_proc_address_mpv(void *ctx, const char *name) {
    (void)ctx;
    return (void *)eglGetProcAddress(name);
}

static void on_mpv_render_update(void *ctx) {
    struct app_state *app = (struct app_state *)ctx;
    app->need_render = true;
}

static void layer_surface_configure(void *data,
                                   struct zwlr_layer_surface_v1 *layer_surface,
                                   uint32_t serial,
                                   uint32_t width,
                                   uint32_t height) {
    struct app_state *app = (struct app_state *)data;
    zwlr_layer_surface_v1_ack_configure(layer_surface, serial);

    if (width > 0 && height > 0) {
        app->width = width;
        app->height = height;
    }

    if (app->egl_window) {
        wl_egl_window_resize(app->egl_window, app->width, app->height, 0, 0);
    }

    app->configured = true;
    app->need_render = true;
}

static void layer_surface_closed(void *data, struct zwlr_layer_surface_v1 *layer_surface) {
    (void)data;
    (void)layer_surface;
    running = 0;
}

static const struct zwlr_layer_surface_v1_listener layer_surface_listener = {
    .configure = layer_surface_configure,
    .closed = layer_surface_closed,
};

static void registry_handler(void *data, struct wl_registry *registry,
                            uint32_t id, const char *interface, uint32_t version) {
    struct app_state *app = (struct app_state *)data;

    if (strcmp(interface, wl_compositor_interface.name) == 0) {
        app->compositor = wl_registry_bind(registry, id, &wl_compositor_interface, version < 4 ? version : 4);
    } else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0) {
        app->layer_shell = wl_registry_bind(registry, id, &zwlr_layer_shell_v1_interface, 1);
    } else if (strcmp(interface, wl_output_interface.name) == 0) {
        if (!app->output) {
            app->output = wl_registry_bind(registry, id, &wl_output_interface, 1);
        }
    }
}

static void registry_remover(void *data, struct wl_registry *registry, uint32_t id) {
    (void)data;
    (void)registry;
    (void)id;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_handler,
    .global_remove = registry_remover,
};

static bool init_egl(struct app_state *app) {
    app->egl_display = eglGetDisplay((EGLNativeDisplayType)app->display);
    if (app->egl_display == EGL_NO_DISPLAY) {
        fprintf(stderr, "Failed to get EGL display\n");
        return false;
    }

    EGLint major, minor;
    if (!eglInitialize(app->egl_display, &major, &minor)) {
        fprintf(stderr, "Failed to initialize EGL\n");
        return false;
    }

    EGLint attribs[] = {
        EGL_SURFACE_TYPE, EGL_WINDOW_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_BIT,
        EGL_NONE
    };

    EGLint num_configs;
    if (!eglChooseConfig(app->egl_display, attribs, &app->egl_config, 1, &num_configs) || num_configs < 1) {
        fprintf(stderr, "Failed to choose EGL config\n");
        return false;
    }

    eglBindAPI(EGL_OPENGL_API);

    EGLint ctx_attribs[] = {
        EGL_CONTEXT_MAJOR_VERSION, 3,
        EGL_CONTEXT_MINOR_VERSION, 0,
        EGL_NONE
    };

    app->egl_context = eglCreateContext(app->egl_display, app->egl_config, EGL_NO_CONTEXT, ctx_attribs);
    if (app->egl_context == EGL_NO_CONTEXT) {
        // Fallback to default version
        app->egl_context = eglCreateContext(app->egl_display, app->egl_config, EGL_NO_CONTEXT, NULL);
    }

    if (app->egl_context == EGL_NO_CONTEXT) {
        fprintf(stderr, "Failed to create EGL context\n");
        return false;
    }

    return true;
}

int main(int argc, char *argv[]) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <video_file> [output_name] [ipc_socket_path]\n", argv[0]);
        return 1;
    }

    const char *video_file = argv[1];
    const char *output_name = argc > 2 ? argv[2] : "";
    const char *ipc_socket = argc > 3 ? argv[3] : NULL;

    signal(SIGINT, sig_handler);
    signal(SIGTERM, sig_handler);

    struct app_state app = {0};
    app.width = 1920;
    app.height = 1080;
    if (output_name && strlen(output_name) > 0) {
        strncpy(app.target_output_name, output_name, sizeof(app.target_output_name) - 1);
    }

    app.display = wl_display_connect(NULL);
    if (!app.display) {
        fprintf(stderr, "Cannot connect to Wayland display\n");
        return 1;
    }

    app.registry = wl_display_get_registry(app.display);
    wl_registry_add_listener(app.registry, &registry_listener, &app);
    wl_display_roundtrip(app.display);

    if (!app.compositor || !app.layer_shell) {
        fprintf(stderr, "Compositor does not support wl_compositor or zwlr_layer_shell_v1\n");
        wl_display_disconnect(app.display);
        return 1;
    }

    if (!init_egl(&app)) {
        wl_display_disconnect(app.display);
        return 1;
    }

    // Create background layer surface
    app.surface = wl_compositor_create_surface(app.compositor);
    app.layer_surface = zwlr_layer_shell_v1_get_layer_surface(
        app.layer_shell,
        app.surface,
        app.output,
        ZWLR_LAYER_SHELL_V1_LAYER_BACKGROUND,
        "wengine-wallpaper"
    );

    // Anchor to all edges to span entire monitor
    zwlr_layer_surface_v1_set_anchor(
        app.layer_surface,
        ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP |
        ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM |
        ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT |
        ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT
    );

    // Exclusive zone -1: true background, never reserve layout space
    zwlr_layer_surface_v1_set_exclusive_zone(app.layer_surface, -1);
    zwlr_layer_surface_v1_set_keyboard_interactivity(app.layer_surface, ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_NONE);
    zwlr_layer_surface_v1_add_listener(app.layer_surface, &layer_surface_listener, &app);

    // Initial commit to request configure event
    wl_surface_commit(app.surface);
    wl_display_roundtrip(app.display);

    // Wait for configure
    while (!app.configured && running) {
        if (wl_display_dispatch(app.display) == -1) break;
    }

    // Create EGL window and surface
    app.egl_window = wl_egl_window_create(app.surface, app.width, app.height);
    app.egl_surface = eglCreateWindowSurface(app.egl_display, app.egl_config, (EGLNativeWindowType)app.egl_window, NULL);
    eglMakeCurrent(app.egl_display, app.egl_surface, app.egl_surface, app.egl_context);

    // Initialize libmpv
    app.mpv = mpv_create();
    if (!app.mpv) {
        fprintf(stderr, "Failed to create mpv instance\n");
        return 1;
    }

    mpv_set_option_string(app.mpv, "vo", "libmpv");
    mpv_set_option_string(app.mpv, "hwdec", "auto-safe");
    mpv_set_option_string(app.mpv, "loop-file", "inf");
    mpv_set_option_string(app.mpv, "image-display-duration", "inf");
    mpv_set_option_string(app.mpv, "keepaspect-window", "no");
    mpv_set_option_string(app.mpv, "video-unscaled", "no");
    mpv_set_option_string(app.mpv, "really-quiet", "");
    mpv_set_option_string(app.mpv, "panscan", "1.0");

    if (ipc_socket && strlen(ipc_socket) > 0) {
        unlink(ipc_socket);
        mpv_set_option_string(app.mpv, "input-ipc-server", ipc_socket);
    }

    if (mpv_initialize(app.mpv) < 0) {
        fprintf(stderr, "Failed to initialize mpv\n");
        return 1;
    }

    mpv_opengl_init_params gl_init_params = {
        .get_proc_address = get_proc_address_mpv,
        .get_proc_address_ctx = NULL,
    };

    int advanced_control = 1;
    mpv_render_param params[] = {
        {MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_API_TYPE_OPENGL},
        {MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &gl_init_params},
        {MPV_RENDER_PARAM_ADVANCED_CONTROL, &advanced_control},
        {0}
    };

    if (mpv_render_context_create(&app.mpv_gl, app.mpv, params) < 0) {
        fprintf(stderr, "Failed to create mpv render context\n");
        return 1;
    }

    mpv_render_context_set_update_callback(app.mpv_gl, on_mpv_render_update, &app);

    // Load file
    const char *cmd[] = {"loadfile", video_file, "replace", NULL};
    mpv_command_async(app.mpv, 0, cmd);

    int wl_fd = wl_display_get_fd(app.display);
    struct pollfd pfd = { .fd = wl_fd, .events = POLLIN };

    while (running) {
        while (wl_display_prepare_read(app.display) != 0) {
            wl_display_dispatch_pending(app.display);
        }
        wl_display_flush(app.display);

        int timeout = app.need_render ? 0 : 20;
        int ret = poll(&pfd, 1, timeout);
        if (ret > 0) {
            wl_display_read_events(app.display);
            wl_display_dispatch_pending(app.display);
        } else {
            wl_display_cancel_read(app.display);
        }

        // Render frame if needed
        if (app.need_render) {
            app.need_render = false;
            uint64_t flags = mpv_render_context_update(app.mpv_gl);
            if (flags & MPV_RENDER_UPDATE_FRAME) {
                mpv_opengl_fbo fbo = {
                    .fbo = 0,
                    .w = app.width,
                    .h = app.height,
                    .internal_format = 0,
                };
                int flip_y = 1;
                mpv_render_param render_params[] = {
                    {MPV_RENDER_PARAM_OPENGL_FBO, &fbo},
                    {MPV_RENDER_PARAM_FLIP_Y, &flip_y},
                    {0}
                };
                mpv_render_context_render(app.mpv_gl, render_params);
                eglSwapBuffers(app.egl_display, app.egl_surface);
            }
        }

        // Handle mpv events
        while (1) {
            mpv_event *event = mpv_wait_event(app.mpv, 0);
            if (event->event_id == MPV_EVENT_NONE) break;
            if (event->event_id == MPV_EVENT_SHUTDOWN) {
                running = 0;
                break;
            }
        }
    }

    // Cleanup
    if (app.mpv_gl) mpv_render_context_free(app.mpv_gl);
    if (app.mpv) mpv_destroy(app.mpv);
    if (ipc_socket) unlink(ipc_socket);
    if (app.egl_surface) eglDestroySurface(app.egl_display, app.egl_surface);
    if (app.egl_context) eglDestroyContext(app.egl_display, app.egl_context);
    if (app.egl_display) eglTerminate(app.egl_display);
    if (app.egl_window) wl_egl_window_destroy(app.egl_window);
    if (app.layer_surface) zwlr_layer_surface_v1_destroy(app.layer_surface);
    if (app.surface) wl_surface_destroy(app.surface);
    if (app.display) wl_display_disconnect(app.display);

    return 0;
}

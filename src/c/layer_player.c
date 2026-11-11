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
#include "xdg-shell.h"

static volatile sig_atomic_t running = 1;

static void sig_handler(int sig) {
    (void)sig;
    running = 0;
}

struct bound_output {
    struct wl_output *output;
    uint32_t id;
    char name[64];
    int32_t x;
    int32_t y;
    int32_t width;
    int32_t height;
    int32_t scale;
    struct bound_output *next;
};

struct app_state {
    struct wl_display *display;
    struct wl_registry *registry;
    struct wl_compositor *compositor;
    struct zwlr_layer_shell_v1 *layer_shell;
    struct xdg_wm_base *xdg_wm_base;

    struct bound_output *outputs;
    struct wl_output *output;
    char target_output_name[64];

    struct wl_surface *surface;
    struct zwlr_layer_surface_v1 *layer_surface;
    struct xdg_surface *xdg_surface;
    struct xdg_toplevel *xdg_toplevel;
    bool is_xdg_mode;

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

/* Layer Shell Callbacks */
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

/* XDG Shell Callbacks */
static void xdg_wm_base_ping(void *data, struct xdg_wm_base *xdg_wm_base, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(xdg_wm_base, serial);
}

static const struct xdg_wm_base_listener xdg_wm_base_listener = {
    .ping = xdg_wm_base_ping,
};

static void xdg_surface_configure(void *data, struct xdg_surface *xdg_surface, uint32_t serial) {
    struct app_state *app = (struct app_state *)data;
    xdg_surface_ack_configure(xdg_surface, serial);

    if (app->egl_window && app->width > 0 && app->height > 0) {
        wl_egl_window_resize(app->egl_window, app->width, app->height, 0, 0);
    }

    app->configured = true;
    app->need_render = true;
}

static const struct xdg_surface_listener xdg_surface_listener = {
    .configure = xdg_surface_configure,
};

static void xdg_toplevel_configure(void *data, struct xdg_toplevel *xdg_toplevel,
                                   int32_t width, int32_t height, struct wl_array *states) {
    (void)xdg_toplevel;
    (void)states;
    struct app_state *app = (struct app_state *)data;
    if (width > 0 && height > 0) {
        app->width = width;
        app->height = height;
    }
}

static void xdg_toplevel_close(void *data, struct xdg_toplevel *xdg_toplevel) {
    (void)data;
    (void)xdg_toplevel;
    running = 0;
}

static const struct xdg_toplevel_listener xdg_toplevel_listener = {
    .configure = xdg_toplevel_configure,
    .close = xdg_toplevel_close,
};

/* Output Callbacks */
static void output_geometry(void *data, struct wl_output *output,
                            int32_t x, int32_t y, int32_t physical_width, int32_t physical_height,
                            int32_t subpixel, const char *make, const char *model, int32_t transform) {
    (void)output; (void)physical_width; (void)physical_height; (void)subpixel; (void)make; (void)model; (void)transform;
    struct bound_output *node = (struct bound_output *)data;
    node->x = x;
    node->y = y;
}

static void output_mode(void *data, struct wl_output *output,
                        uint32_t flags, int32_t width, int32_t height, int32_t refresh) {
    (void)output; (void)refresh;
    struct bound_output *node = (struct bound_output *)data;
    if (flags & WL_OUTPUT_MODE_CURRENT) {
        node->width = width;
        node->height = height;
    }
}

static void output_done(void *data, struct wl_output *output) {
    (void)data;
    (void)output;
}

static void output_scale(void *data, struct wl_output *output, int32_t factor) {
    (void)output;
    struct bound_output *node = (struct bound_output *)data;
    if (factor > 0) {
        node->scale = factor;
    }
}

static void output_name(void *data, struct wl_output *output, const char *name) {
    (void)output;
    struct bound_output *node = (struct bound_output *)data;
    if (name) {
        strncpy(node->name, name, sizeof(node->name) - 1);
    }
}

static void output_description(void *data, struct wl_output *output, const char *description) {
    (void)data;
    (void)output;
    (void)description;
}

static const struct wl_output_listener output_listener = {
    .geometry = output_geometry,
    .mode = output_mode,
    .done = output_done,
    .scale = output_scale,
    .name = output_name,
    .description = output_description,
};

/* Registry Callbacks */
static void registry_handler(void *data, struct wl_registry *registry,
                             uint32_t id, const char *interface, uint32_t version) {
    struct app_state *app = (struct app_state *)data;

    if (strcmp(interface, wl_compositor_interface.name) == 0) {
        app->compositor = wl_registry_bind(registry, id, &wl_compositor_interface, version < 4 ? version : 4);
    } else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0) {
        app->layer_shell = wl_registry_bind(registry, id, &zwlr_layer_shell_v1_interface, 1);
    } else if (strcmp(interface, xdg_wm_base_interface.name) == 0) {
        app->xdg_wm_base = wl_registry_bind(registry, id, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(app->xdg_wm_base, &xdg_wm_base_listener, app);
    } else if (strcmp(interface, wl_output_interface.name) == 0) {
        uint32_t bind_ver = version < 4 ? version : 4;
        struct wl_output *out = wl_registry_bind(registry, id, &wl_output_interface, bind_ver);
        struct bound_output *node = calloc(1, sizeof(struct bound_output));
        node->output = out;
        node->id = id;
        node->scale = 1;
        node->next = app->outputs;
        app->outputs = node;
        wl_output_add_listener(out, &output_listener, node);
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

    // Initial roundtrip to bind globals
    wl_display_roundtrip(app.display);
    // Second roundtrip to receive output events (name, geometry, mode)
    wl_display_roundtrip(app.display);

    if (!app.compositor) {
        fprintf(stderr, "Compositor does not support wl_compositor\n");
        wl_display_disconnect(app.display);
        return 1;
    }

    if (!app.layer_shell && !app.xdg_wm_base) {
        fprintf(stderr, "Compositor supports neither zwlr_layer_shell_v1 nor xdg_wm_base\n");
        wl_display_disconnect(app.display);
        return 1;
    }

    // Resolve matching output if specified
    struct wl_output *chosen_output = NULL;
    if (app.target_output_name[0] != '\0' && strcmp(app.target_output_name, "*") != 0) {
        struct bound_output *cur = app.outputs;
        while (cur) {
            if (strcmp(cur->name, app.target_output_name) == 0) {
                chosen_output = cur->output;
                if (cur->width > 0 && cur->height > 0) {
                    app.width = cur->width;
                    app.height = cur->height;
                }
                break;
            }
            cur = cur->next;
        }

        // Try numeric index fallback
        if (!chosen_output) {
            char *endptr = NULL;
            long idx = strtol(app.target_output_name, &endptr, 10);
            if (endptr && *endptr == '\0' && idx >= 0) {
                cur = app.outputs;
                long i = 0;
                while (cur) {
                    if (i == idx) {
                        chosen_output = cur->output;
                        if (cur->width > 0 && cur->height > 0) {
                            app.width = cur->width;
                            app.height = cur->height;
                        }
                        break;
                    }
                    i++;
                    cur = cur->next;
                }
            }
        }

        // If specific target was not matched and only 1 output is connected, fallback to it
        if (!chosen_output && app.outputs && !app.outputs->next) {
            chosen_output = app.outputs->output;
            if (app.outputs->width > 0 && app.outputs->height > 0) {
                app.width = app.outputs->width;
                app.height = app.outputs->height;
            }
        }
    }

    app.output = chosen_output;
    if (app.output) {
        fprintf(stderr, "wengine-layer-player: Targeting display '%s' (%dx%d)\n",
                app.target_output_name, app.width, app.height);
    } else {
        fprintf(stderr, "wengine-layer-player: Spanning default/all displays\n");
    }

    if (!init_egl(&app)) {
        wl_display_disconnect(app.display);
        return 1;
    }

    // Create wayland surface
    app.surface = wl_compositor_create_surface(app.compositor);

    if (app.layer_shell) {
        app.is_xdg_mode = false;
        app.layer_surface = zwlr_layer_shell_v1_get_layer_surface(
            app.layer_shell,
            app.surface,
            app.output,
            ZWLR_LAYER_SHELL_V1_LAYER_BACKGROUND,
            "wengine-wallpaper"
        );

        // Anchor to all edges to span entire display
        zwlr_layer_surface_v1_set_anchor(
            app.layer_surface,
            ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP |
            ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM |
            ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT |
            ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT
        );
        zwlr_layer_surface_v1_set_size(app.layer_surface, 0, 0);

        // Exclusive zone -1: true background layer, never reserve layout space
        zwlr_layer_surface_v1_set_exclusive_zone(app.layer_surface, -1);
        zwlr_layer_surface_v1_set_keyboard_interactivity(app.layer_surface, ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_NONE);
        zwlr_layer_surface_v1_add_listener(app.layer_surface, &layer_surface_listener, &app);
    } else {
        app.is_xdg_mode = true;
        fprintf(stderr, "wengine-layer-player: zwlr_layer_shell_v1 unavailable, using xdg_wm_base fallback\n");

        app.xdg_surface = xdg_wm_base_get_xdg_surface(app.xdg_wm_base, app.surface);
        xdg_surface_add_listener(app.xdg_surface, &xdg_surface_listener, &app);

        app.xdg_toplevel = xdg_surface_get_toplevel(app.xdg_surface);
        xdg_toplevel_add_listener(app.xdg_toplevel, &xdg_toplevel_listener, &app);
        xdg_toplevel_set_app_id(app.xdg_toplevel, "wengine-wallpaper");
        xdg_toplevel_set_title(app.xdg_toplevel, "wengine-wallpaper");
        xdg_toplevel_set_fullscreen(app.xdg_toplevel, app.output);
    }

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
    mpv_set_option_string(app.mpv, "scale", "spline36");
    mpv_set_option_string(app.mpv, "cscale", "spline36");
    mpv_set_option_string(app.mpv, "dscale", "mitchell");
    mpv_set_option_string(app.mpv, "correct-pts", "yes");

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

    // Load media file
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
    if (app.xdg_toplevel) xdg_toplevel_destroy(app.xdg_toplevel);
    if (app.xdg_surface) xdg_surface_destroy(app.xdg_surface);
    if (app.xdg_wm_base) xdg_wm_base_destroy(app.xdg_wm_base);
    if (app.layer_shell) zwlr_layer_shell_v1_destroy(app.layer_shell);
    if (app.surface) wl_surface_destroy(app.surface);

    while (app.outputs) {
        struct bound_output *next = app.outputs->next;
        wl_output_destroy(app.outputs->output);
        free(app.outputs);
        app.outputs = next;
    }

    if (app.registry) wl_registry_destroy(app.registry);
    if (app.compositor) wl_compositor_destroy(app.compositor);
    if (app.display) wl_display_disconnect(app.display);

    return 0;
}

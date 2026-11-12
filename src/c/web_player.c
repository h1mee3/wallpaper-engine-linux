#define _GNU_SOURCE
#include <gtk/gtk.h>
#include <gtk-layer-shell.h>
#include <webkit2/webkit2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <unistd.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <fcntl.h>
#include <errno.h>

static GtkWidget *main_window = NULL;
static GtkWidget *web_view = NULL;
static char *ipc_socket_path = NULL;
static bool is_paused = false;
static bool is_muted = false;
static int current_volume = 100;

static void handle_pause(void) {
    if (is_paused) return;
    is_paused = true;
    fprintf(stderr, "wengine-web-player: Pausing wallpaper playback\n");

    if (web_view) {
        const char *js = 
            "if (document.hidden !== undefined) {"
            "  Object.defineProperty(document, 'hidden', {value: true, writable: true});"
            "  document.dispatchEvent(new Event('visibilitychange'));"
            "}"
            "document.querySelectorAll('video, audio').forEach(function(el){ el.pause(); });"
            "if (window.onWallpaperPause) window.onWallpaperPause();";
        webkit_web_view_run_javascript(WEBKIT_WEB_VIEW(web_view), js, NULL, NULL, NULL);
    }
}

static void handle_resume(void) {
    if (!is_paused) return;
    is_paused = false;
    fprintf(stderr, "wengine-web-player: Resuming wallpaper playback\n");

    if (main_window) {
        gtk_widget_show_all(main_window);
    }

    if (web_view) {
        const char *js = 
            "if (document.hidden !== undefined) {"
            "  Object.defineProperty(document, 'hidden', {value: false, writable: true});"
            "  document.dispatchEvent(new Event('visibilitychange'));"
            "}"
            "document.querySelectorAll('video, audio').forEach(function(el){ el.play(); });"
            "if (window.onWallpaperResume) window.onWallpaperResume();";
        webkit_web_view_run_javascript(WEBKIT_WEB_VIEW(web_view), js, NULL, NULL, NULL);
    }
}

static void handle_mute(bool mute) {
    is_muted = mute;
    if (web_view) {
        char js[512];
        snprintf(js, sizeof(js),
            "document.querySelectorAll('video, audio').forEach(function(el){ el.muted = %s; });"
            "if (window.Sakana && window.Sakana.setMute) window.Sakana.setMute(%s);",
            mute ? "true" : "false",
            mute ? "true" : "false"
        );
        webkit_web_view_run_javascript(WEBKIT_WEB_VIEW(web_view), js, NULL, NULL, NULL);
    }
}

static void handle_volume(int vol) {
    if (vol < 0) vol = 0;
    if (vol > 100) vol = 100;
    current_volume = vol;
    float norm = (float)vol / 100.0f;
    if (web_view) {
        char js[256];
        snprintf(js, sizeof(js),
            "document.querySelectorAll('video, audio').forEach(function(el){ el.volume = %.2f; });",
            norm
        );
        webkit_web_view_run_javascript(WEBKIT_WEB_VIEW(web_view), js, NULL, NULL, NULL);
    }
}

static void sig_handler(int sig) {
    if (sig == SIGTERM || sig == SIGINT) {
        if (ipc_socket_path) {
            unlink(ipc_socket_path);
        }
        gtk_main_quit();
    }
}

/* Parse and execute JSON or plain text IPC commands */
static void process_ipc_command(const char *cmd, int client_fd) {
    const char *resp = "{\"status\":\"ok\"}\n";

    if (strstr(cmd, "pause") != NULL) {
        handle_pause();
    } else if (strstr(cmd, "resume") != NULL) {
        handle_resume();
    } else if (strstr(cmd, "toggle") != NULL) {
        if (is_paused) {
            handle_resume();
        } else {
            handle_pause();
        }
    } else if (strstr(cmd, "mute") != NULL) {
        bool m = strstr(cmd, "false") == NULL;
        handle_mute(m);
    } else if (strstr(cmd, "volume") != NULL) {
        int v = 100;
        const char *p = strstr(cmd, "value");
        if (p) {
            sscanf(p, "value\": %d", &v);
        }
        handle_volume(v);
    } else if (strstr(cmd, "stop") != NULL) {
        if (ipc_socket_path) unlink(ipc_socket_path);
        write(client_fd, resp, strlen(resp));
        close(client_fd);
        gtk_main_quit();
        return;
    }

    write(client_fd, resp, strlen(resp));
}

static gboolean on_ipc_io(GIOChannel *source, GIOCondition condition, gpointer data) {
    (void)source;
    (void)condition;
    int server_fd = GPOINTER_TO_INT(data);
    struct sockaddr_un client_addr;
    socklen_t client_len = sizeof(client_addr);

    int client_fd = accept(server_fd, (struct sockaddr *)&client_addr, &client_len);
    if (client_fd < 0) {
        return TRUE;
    }

    char buffer[2048];
    ssize_t n = read(client_fd, buffer, sizeof(buffer) - 1);
    if (n > 0) {
        buffer[n] = '\0';
        process_ipc_command(buffer, client_fd);
    }

    close(client_fd);
    return TRUE;
}

static void setup_ipc_socket(const char *socket_path) {
    if (!socket_path || strlen(socket_path) == 0) return;

    unlink(socket_path);
    int server_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd < 0) {
        perror("socket");
        return;
    }

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, socket_path, sizeof(addr.sun_path) - 1);

    if (bind(server_fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("bind");
        close(server_fd);
        return;
    }

    if (listen(server_fd, 5) < 0) {
        perror("listen");
        close(server_fd);
        return;
    }

    int flags = fcntl(server_fd, F_GETFL, 0);
    fcntl(server_fd, F_SETFL, flags | O_NONBLOCK);

    ipc_socket_path = strdup(socket_path);

    GIOChannel *channel = g_io_channel_unix_new(server_fd);
    g_io_add_watch(channel, G_IO_IN, on_ipc_io, GINT_TO_POINTER(server_fd));
    g_io_channel_unref(channel);
}

static gboolean on_button_press(GtkWidget *widget, GdkEventButton *event, gpointer user_data) {
    (void)widget;
    (void)user_data;
    fprintf(stderr, "wengine-web-player: Button press at (%.1f, %.1f) button=%u\n", event->x, event->y, event->button);
    return FALSE; // Allow WebKit to process the click and drag
}

static gboolean on_button_release(GtkWidget *widget, GdkEventButton *event, gpointer user_data) {
    (void)widget;
    (void)user_data;
    fprintf(stderr, "wengine-web-player: Button release at (%.1f, %.1f) button=%u\n", event->x, event->y, event->button);
    return FALSE;
}

int main(int argc, char *argv[]) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <html_file_or_url> [monitor_name] [ipc_socket_path] [layer]\n", argv[0]);
        return 1;
    }

    const char *url_or_path = argv[1];
    const char *monitor_name = (argc > 2) ? argv[2] : "";
    const char *ipc_socket = (argc > 3) ? argv[3] : NULL;
    const char *layer_str = (argc > 4) ? argv[4] : "bottom";

    signal(SIGINT, sig_handler);
    signal(SIGTERM, sig_handler);

    // Prefer native Wayland backend across all Wayland DEs/WMs
    gdk_set_allowed_backends("wayland,x11");
    gtk_init(&argc, &argv);

    // Auto-detect Sakana widget and ensure transparent alpha parameter is supplied
    char adjusted_path[4096];
    strncpy(adjusted_path, url_or_path, sizeof(adjusted_path) - 1);
    adjusted_path[sizeof(adjusted_path) - 1] = '\0';
    if (strstr(url_or_path, "Sakana.html") != NULL && strstr(url_or_path, "alpha=") == NULL) {
        if (strchr(url_or_path, '?') != NULL) {
            snprintf(adjusted_path, sizeof(adjusted_path), "%s&alpha=true", url_or_path);
        } else {
            snprintf(adjusted_path, sizeof(adjusted_path), "%s?alpha=true", url_or_path);
        }
    }

    // Build URL (ensure file:// protocol if local file path)
    char full_url[4096];
    if (strstr(adjusted_path, "://") == NULL) {
        if (adjusted_path[0] == '/') {
            snprintf(full_url, sizeof(full_url), "file://%s", adjusted_path);
        } else {
            char cwd[2048];
            if (getcwd(cwd, sizeof(cwd))) {
                snprintf(full_url, sizeof(full_url), "file://%s/%s", cwd, adjusted_path);
            } else {
                snprintf(full_url, sizeof(full_url), "file://%s", adjusted_path);
            }
        }
    } else {
        strncpy(full_url, adjusted_path, sizeof(full_url) - 1);
        full_url[sizeof(full_url) - 1] = '\0';
    }

    main_window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(main_window), "wengine-wallpaper");
    gtk_window_set_decorated(GTK_WINDOW(main_window), FALSE);

    // Resolve target monitor if requested (match connector e.g. DP-1, plug name, model, or index)
    GdkDisplay *display = gdk_display_get_default();
    GdkScreen *screen = gdk_screen_get_default();
    GdkMonitor *target_mon = NULL;
    int target_mon_idx = -1;

    if (monitor_name && strlen(monitor_name) > 0 && strcmp(monitor_name, "*") != 0 && display) {
        int n_monitors = gdk_display_get_n_monitors(display);
        for (int i = 0; i < n_monitors; i++) {
            GdkMonitor *mon = gdk_display_get_monitor(display, i);
            const char *model = gdk_monitor_get_model(mon);
            char *plug = screen ? gdk_screen_get_monitor_plug_name(screen, i) : NULL;
            char *endptr = NULL;
            long idx = strtol(monitor_name, &endptr, 10);
            bool matched = false;

            if (endptr && *endptr == '\0' && idx == i) {
                matched = true;
            } else if (plug && strcmp(plug, monitor_name) == 0) {
                matched = true;
            } else if (model && strstr(model, monitor_name)) {
                matched = true;
            }

            if (plug) g_free(plug);

            if (matched) {
                target_mon = mon;
                target_mon_idx = i;
                break;
            }
        }
    }

    gboolean layer_supported = gtk_layer_is_supported();
    GtkLayerShellLayer layer = GTK_LAYER_SHELL_LAYER_BOTTOM;
    if (layer_str && strcasecmp(layer_str, "background") == 0) {
        layer = GTK_LAYER_SHELL_LAYER_BACKGROUND;
    }

    if (layer_supported) {
        gtk_layer_init_for_window(GTK_WINDOW(main_window));

        // Configure Layer: Default BOTTOM for interactive wallpapers so they receive mouse clicks behind windows
        gtk_layer_set_layer(GTK_WINDOW(main_window), layer);
        gtk_layer_set_namespace(GTK_WINDOW(main_window), "wengine-wallpaper");

        // Anchor to all 4 edges to span display
        gtk_layer_set_anchor(GTK_WINDOW(main_window), GTK_LAYER_SHELL_EDGE_LEFT, TRUE);
        gtk_layer_set_anchor(GTK_WINDOW(main_window), GTK_LAYER_SHELL_EDGE_RIGHT, TRUE);
        gtk_layer_set_anchor(GTK_WINDOW(main_window), GTK_LAYER_SHELL_EDGE_TOP, TRUE);
        gtk_layer_set_anchor(GTK_WINDOW(main_window), GTK_LAYER_SHELL_EDGE_BOTTOM, TRUE);

        // Exclusive zone -1: true desktop layer, never reserve window panel space
        gtk_layer_set_exclusive_zone(GTK_WINDOW(main_window), -1);

        // Keyboard mode none: do not steal keyboard focus from workspace windows
        gtk_layer_set_keyboard_mode(GTK_WINDOW(main_window), GTK_LAYER_SHELL_KEYBOARD_MODE_NONE);

        if (target_mon) {
            gtk_layer_set_monitor(GTK_WINDOW(main_window), target_mon);
        }
    } else {
        // Fallback for Wayland compositors without zwlr_layer_shell_v1 (e.g. GNOME / Mutter)
        fprintf(stderr, "wengine-web-player: gtk-layer-shell unsupported on this compositor, using fullscreen fallback\n");
        gtk_window_set_skip_taskbar_hint(GTK_WINDOW(main_window), TRUE);
        gtk_window_set_skip_pager_hint(GTK_WINDOW(main_window), TRUE);
        gtk_window_set_keep_below(GTK_WINDOW(main_window), TRUE);
        if (screen && target_mon_idx >= 0) {
            gtk_window_fullscreen_on_monitor(GTK_WINDOW(main_window), screen, target_mon_idx);
        } else {
            gtk_window_fullscreen(GTK_WINDOW(main_window));
        }
    }

    // Create WebKit Web View with unrestricted media autoplay policy
    WebKitWebsitePolicies *policies = webkit_website_policies_new_with_policies(
        "autoplay", WEBKIT_AUTOPLAY_ALLOW,
        NULL
    );
    web_view = g_object_new(WEBKIT_TYPE_WEB_VIEW, "website-policies", policies, NULL);

    // Enable transparent background so wallpaper blends seamlessly
    GdkRGBA transparent = {0.0, 0.0, 0.0, 0.0};
    webkit_web_view_set_background_color(WEBKIT_WEB_VIEW(web_view), &transparent);

    WebKitSettings *settings = webkit_web_view_get_settings(WEBKIT_WEB_VIEW(web_view));
    webkit_settings_set_enable_webgl(settings, TRUE);
    webkit_settings_set_enable_javascript(settings, TRUE);
    webkit_settings_set_enable_media_stream(settings, TRUE);
    webkit_settings_set_media_playback_allows_inline(settings, TRUE);
    webkit_settings_set_enable_smooth_scrolling(settings, TRUE);
    webkit_settings_set_allow_file_access_from_file_urls(settings, TRUE);
    webkit_settings_set_allow_universal_access_from_file_urls(settings, TRUE);
    webkit_settings_set_enable_write_console_messages_to_stdout(settings, TRUE);

    // Inject Wallpaper Engine API Compatibility Shim & auto alpha transparency
    WebKitUserContentManager *cm = webkit_web_view_get_user_content_manager(WEBKIT_WEB_VIEW(web_view));
    const char *we_shim = 
        "(function() {\n"
        "  var _userProps = {};\n"
        "  try {\n"
        "    var xhr = new XMLHttpRequest();\n"
        "    xhr.open('GET', 'project.json', false);\n"
        "    xhr.send();\n"
        "    if (xhr.status === 200 || xhr.status === 0) {\n"
        "      var data = JSON.parse(xhr.responseText);\n"
        "      if (data && data.general && data.general.properties) {\n"
        "        _userProps = data.general.properties;\n"
        "      }\n"
        "    }\n"
        "  } catch (e) {}\n"
        "  var _wpl = {};\n"
        "  function notifyListener(l) {\n"
        "    if (!l) return;\n"
        "    try {\n"
        "      if (typeof l.applyGeneralProperties === 'function') l.applyGeneralProperties({ fps: 60 });\n"
        "      if (typeof l.applyUserProperties === 'function') l.applyUserProperties(_userProps);\n"
        "      if (typeof l.setPaused === 'function') l.setPaused(false);\n"
        "    } catch (err) {}\n"
        "  }\n"
        "  Object.defineProperty(window, 'wallpaperPropertyListener', {\n"
        "    get: function() { return _wpl; },\n"
        "    set: function(val) {\n"
        "      _wpl = val;\n"
        "      setTimeout(function() { notifyListener(_wpl); }, 20);\n"
        "    },\n"
        "    configurable: true,\n"
        "    enumerable: true\n"
        "  });\n"
        "  window.wallpaperRegisterAudioListener = window.wallpaperRegisterAudioListener || function(cb) {};\n"
        "  window.wallpaperRegisterMediaListener = window.wallpaperRegisterMediaListener || function(cb) {};\n"
        "  window.wallpaperRequestRandomFileForProperty = window.wallpaperRequestRandomFileForProperty || function(p, cb) {};\n"
        "  document.addEventListener('DOMContentLoaded', function() {\n"
        "    if (document.documentElement && !document.documentElement.getAttribute('data-alpha')) {\n"
        "      document.documentElement.setAttribute('data-alpha', 'true');\n"
        "    }\n"
        "    if (window.wallpaperPropertyListener) notifyListener(window.wallpaperPropertyListener);\n"
        "  });\n"
        "  window.addEventListener('load', function() {\n"
        "    if (window.wallpaperPropertyListener) notifyListener(window.wallpaperPropertyListener);\n"
        "  });\n"
        "})();\n";
    WebKitUserScript *script = webkit_user_script_new(
        we_shim,
        WEBKIT_USER_CONTENT_INJECT_TOP_FRAME,
        WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START,
        NULL, NULL
    );
    webkit_user_content_manager_add_script(cm, script);
    webkit_user_script_unref(script);

    // Enable full pointer and gesture events for interactive/reactive physics
    gtk_widget_add_events(main_window,
        GDK_BUTTON_PRESS_MASK |
        GDK_BUTTON_RELEASE_MASK |
        GDK_POINTER_MOTION_MASK |
        GDK_SCROLL_MASK |
        GDK_SMOOTH_SCROLL_MASK |
        GDK_TOUCH_MASK
    );
    gtk_widget_add_events(web_view,
        GDK_BUTTON_PRESS_MASK |
        GDK_BUTTON_RELEASE_MASK |
        GDK_POINTER_MOTION_MASK |
        GDK_SCROLL_MASK |
        GDK_SMOOTH_SCROLL_MASK |
        GDK_TOUCH_MASK
    );

    g_signal_connect(web_view, "button-press-event", G_CALLBACK(on_button_press), NULL);
    g_signal_connect(web_view, "button-release-event", G_CALLBACK(on_button_release), NULL);

    gtk_container_add(GTK_CONTAINER(main_window), web_view);

    // Setup IPC socket
    if (ipc_socket) {
        setup_ipc_socket(ipc_socket);
    }

    fprintf(stderr, "wengine-web-player: Loading interactive wallpaper '%s' on layer %s\n",
            full_url, !layer_supported ? "FALLBACK" : ((layer == GTK_LAYER_SHELL_LAYER_BOTTOM) ? "BOTTOM" : "BACKGROUND"));
    webkit_web_view_load_uri(WEBKIT_WEB_VIEW(web_view), full_url);

    gtk_widget_show_all(main_window);
    gtk_main();

    if (ipc_socket_path) {
        unlink(ipc_socket_path);
        free(ipc_socket_path);
    }

    return 0;
}

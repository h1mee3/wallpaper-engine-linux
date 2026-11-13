#define _GNU_SOURCE
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
#include <stdio.h>
#include <stdlib.h>
#include <signal.h>

static void sig_handler(int sig) {
    if (sig == SIGTERM || sig == SIGINT) {
        gtk_main_quit();
    }
}

static gboolean on_context_menu(
    WebKitWebView *web_view,
    WebKitContextMenu *context_menu,
    GdkEvent *event,
    WebKitHitTestResult *hit_test_result,
    gpointer user_data
) {
    (void)web_view;
    (void)context_menu;
    (void)event;
    (void)hit_test_result;
    (void)user_data;
    return TRUE;
}

static void on_window_destroy(GtkWidget *widget, gpointer data) {
    (void)widget;
    (void)data;
    gtk_main_quit();
}

int main(int argc, char *argv[]) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <url>\n", argv[0]);
        return 1;
    }

    const char *url = argv[1];

    signal(SIGTERM, sig_handler);
    signal(SIGINT, sig_handler);

    g_set_prgname("wengine");
    gtk_init(&argc, &argv);

    GtkWidget *window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(window), "Wallpaper Engine — wengine");
    gtk_window_set_icon_name(GTK_WINDOW(window), "wengine");
    gtk_window_set_default_size(GTK_WINDOW(window), 1280, 800);
    gtk_widget_set_size_request(window, 860, 560);
    gtk_window_set_position(GTK_WINDOW(window), GTK_WIN_POS_CENTER);

    /* Dark background to avoid white flash while loading */
    GdkRGBA bg = { 0.06, 0.08, 0.11, 1.0 };
    G_GNUC_BEGIN_IGNORE_DEPRECATIONS
    gtk_widget_override_background_color(window, GTK_STATE_FLAG_NORMAL, &bg);
    G_GNUC_END_IGNORE_DEPRECATIONS

    WebKitWebView *web_view = WEBKIT_WEB_VIEW(webkit_web_view_new());
    webkit_web_view_set_background_color(web_view, &bg);

    WebKitSettings *settings = webkit_web_view_get_settings(web_view);
    webkit_settings_set_enable_javascript(settings, TRUE);
    webkit_settings_set_enable_smooth_scrolling(settings, TRUE);
    webkit_settings_set_hardware_acceleration_policy(
        settings, WEBKIT_HARDWARE_ACCELERATION_POLICY_ALWAYS);

    g_signal_connect(window, "destroy", G_CALLBACK(on_window_destroy), NULL);
    g_signal_connect(web_view, "context-menu", G_CALLBACK(on_context_menu), NULL);

    gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(web_view));
    webkit_web_view_load_uri(web_view, url);

    gtk_widget_show_all(window);
    gtk_main();

    return 0;
}

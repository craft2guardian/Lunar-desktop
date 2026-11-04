#include <gtk/gtk.h>
#include <gdk/wayland/gdkwayland.h>
#include <wayland-client.h>
#include <wayland-client-protocol.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* KWin's Plasma shell protocol is used for desktop/panel surfaces. */
static const struct wl_interface plasma_surface_interface;

static const struct wl_interface *shell_surface_types[] = {
    &plasma_surface_interface,
    &wl_surface_interface,
};
static const struct wl_message shell_requests[] = {
    { "get_surface", "no", shell_surface_types },
};
static const struct wl_interface plasma_shell_interface = {
    "org_kde_plasma_shell", 8,
    1, shell_requests,
    0, NULL,
};

static const struct wl_interface *surface_output_types[] = {
    &wl_output_interface,
};
static const struct wl_message plasma_surface_requests[] = {
    { "destroy", "", NULL },
    { "set_output", "o", surface_output_types },
    { "set_position", "ii", NULL },
    { "set_role", "u", NULL },
    { "set_panel_behavior", "u", NULL },
    { "set_skip_taskbar", "u", NULL },
    { "panel_auto_hide_hide", "", NULL },
    { "panel_auto_hide_show", "", NULL },
    { "set_panel_takes_focus", "u", NULL },
    { "set_skip_switcher", "u", NULL },
    { "open_under_cursor", "", NULL },
};
static const struct wl_message plasma_surface_events[] = {
    { "auto_hidden_panel_hidden", "", NULL },
    { "auto_hidden_panel_shown", "", NULL },
};
static const struct wl_interface plasma_surface_interface = {
    "org_kde_plasma_surface", 8,
    11, plasma_surface_requests,
    2, plasma_surface_events,
};

struct registry_state {
    uint32_t shell_name;
    uint32_t shell_version;
};

struct shell_handle {
    struct wl_proxy *surface_role;
};

static struct wl_display *plasma_display;
static struct wl_proxy *plasma_shell;

static void registry_global(void *data, struct wl_registry *registry,
                            uint32_t name, const char *interface,
                            uint32_t version) {
    (void) registry;
    struct registry_state *state = data;
    if (strcmp(interface, "org_kde_plasma_shell") == 0) {
        state->shell_name = name;
        state->shell_version = version;
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry,
                                   uint32_t name) {
    (void) data;
    (void) registry;
    (void) name;
}

static const struct wl_registry_listener registry_listener = {
    registry_global,
    registry_global_remove,
};

static struct wl_proxy *ensure_plasma_shell(struct wl_display *display) {
    if (plasma_shell) {
        return plasma_display == display ? plasma_shell : NULL;
    }

    struct wl_event_queue *queue = wl_display_create_queue(display);
    if (!queue) return NULL;
    struct wl_registry *registry = wl_display_get_registry(display);
    if (!registry) {
        wl_event_queue_destroy(queue);
        return NULL;
    }
    wl_proxy_set_queue((struct wl_proxy *) registry, queue);

    struct registry_state state = {0};
    wl_registry_add_listener(registry, &registry_listener, &state);
    if (wl_display_roundtrip_queue(display, queue) < 0 || !state.shell_name) {
        wl_registry_destroy(registry);
        wl_event_queue_destroy(queue);
        return NULL;
    }

    uint32_t version = state.shell_version < 8 ? state.shell_version : 8;
    struct wl_proxy *shell = wl_registry_bind(
        registry, state.shell_name, &plasma_shell_interface, version);
    if (shell) {
        /* Keep the singleton on GDK's default queue after the temporary
         * registry queue is destroyed. */
        wl_proxy_set_queue(shell, NULL);
    }
    wl_registry_destroy(registry);
    wl_event_queue_destroy(queue);
    if (!shell) return NULL;

    plasma_display = display;
    plasma_shell = shell;
    wl_display_flush(display);
    return plasma_shell;
}

int lunar_kde_shell_is_supported(GdkDisplay *gdk_display) {
    if (!gdk_display || !GDK_IS_WAYLAND_DISPLAY(gdk_display)) return 0;
    struct wl_display *display = gdk_wayland_display_get_wl_display(gdk_display);
    return display && ensure_plasma_shell(display) ? 1 : 0;
}

void *lunar_kde_shell_attach(GtkWindow *window, GdkMonitor *monitor,
                              uint32_t role, int32_t x, int32_t y) {
    if (!window || !monitor) return NULL;
    GdkSurface *gdk_surface = gtk_native_get_surface(GTK_NATIVE(window));
    if (!gdk_surface ||
        !GDK_IS_WAYLAND_DISPLAY(gdk_surface_get_display(gdk_surface))) {
        return NULL;
    }

    struct wl_display *display = gdk_wayland_display_get_wl_display(
        gdk_surface_get_display(gdk_surface));
    struct wl_surface *surface = gdk_wayland_surface_get_wl_surface(gdk_surface);
    struct wl_output *output = gdk_wayland_monitor_get_wl_output(monitor);
    struct wl_proxy *shell = ensure_plasma_shell(display);
    if (!surface || !output || !shell) return NULL;

    struct wl_proxy *plasma_surface = wl_proxy_marshal_flags(
        shell, 0, &plasma_surface_interface, wl_proxy_get_version(shell), 0,
        NULL, surface);
    if (!plasma_surface) return NULL;

    uint32_t version = wl_proxy_get_version(plasma_surface);
    wl_proxy_marshal_flags(plasma_surface, 3, NULL, version, 0, role);
    wl_proxy_marshal_flags(plasma_surface, 1, NULL, version, 0, output);
    wl_proxy_marshal_flags(plasma_surface, 2, NULL, version, 0, x, y);
    if (version >= 2) {
        wl_proxy_marshal_flags(plasma_surface, 5, NULL, version, 0, 1u);
    }
    if (version >= 5) {
        wl_proxy_marshal_flags(plasma_surface, 9, NULL, version, 0, 1u);
    }
    wl_display_flush(display);

    struct shell_handle *handle = malloc(sizeof(*handle));
    if (!handle) {
        wl_proxy_marshal_flags(plasma_surface, 0, NULL, version,
                               WL_MARSHAL_FLAG_DESTROY);
        return NULL;
    }
    handle->surface_role = plasma_surface;
    return handle;
}

void lunar_kde_shell_detach(void *opaque) {
    struct shell_handle *handle = opaque;
    if (!handle) return;
    wl_proxy_marshal_flags(handle->surface_role, 0, NULL,
                           wl_proxy_get_version(handle->surface_role),
                           WL_MARSHAL_FLAG_DESTROY);
    free(handle);
}

void lunar_kde_shell_set_position(void *opaque, int32_t x, int32_t y) {
    struct shell_handle *handle = opaque;
    if (!handle || !handle->surface_role) return;
    wl_proxy_marshal_flags(handle->surface_role, 2, NULL,
                           wl_proxy_get_version(handle->surface_role), 0,
                           x, y);
    if (plasma_display) wl_display_flush(plasma_display);
}

/* GTK4 exposes no xdg_toplevel handle, so size limits can't be sent to the
 * compositor directly. The minimum size is set GTK-side via
 * gtk_widget_set_size_request() (see settings.rs); a maximum width cannot be
 * enforced this way. Kept as a no-op so the Rust caller still links. */
void lunar_toplevel_set_size_limits(GdkSurface *surface, int32_t min_width,
                                     int32_t min_height, int32_t max_width,
                                     int32_t max_height) {
    (void)surface; (void)min_width; (void)min_height;
    (void)max_width; (void)max_height;
}

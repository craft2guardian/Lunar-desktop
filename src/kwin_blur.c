#include <gtk/gtk.h>
#include <gdk/wayland/gdkwayland.h>
#include <wayland-client.h>
#include <wayland-client-protocol.h>

#include "ext-background-effect-v1-client-protocol.h"

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct blur_handle {
    struct ext_background_effect_surface_v1 *effect;
    struct wl_surface *surface;
    struct wl_compositor *compositor;
    struct wl_display *display;
};

struct registry_state {
    uint32_t manager_name;
    uint32_t compositor_name;
    uint32_t manager_version;
    uint32_t compositor_version;
};

struct capability_state {
    uint32_t flags;
};

static void report_blur_unavailable(const char *reason) {
    /* Every popover and window asks once; one line is enough. */
    static int reported = 0;
    if (reported++) return;
    fprintf(stderr, "lunar-desktop: compositor backdrop blur protocol unavailable: %s\n", reason);
}

static void report_protocol_error(struct wl_display *display, const char *stage) {
    const struct wl_interface *interface = NULL;
    uint32_t object_id = 0;
    uint32_t code = wl_display_get_protocol_error(display, &interface, &object_id);
    if (code) {
        fprintf(stderr,
                "lunar-desktop: KWin backdrop blur protocol error at %s: %s object=%u code=%u\n",
                stage, interface ? interface->name : "unknown", object_id, code);
    } else {
        fprintf(stderr, "lunar-desktop: KWin backdrop blur failed at %s\n", stage);
    }
}

static void registry_global(void *data, struct wl_registry *registry,
                            uint32_t name, const char *interface,
                            uint32_t version) {
    (void) registry;
    struct registry_state *state = data;
    if (strcmp(interface, ext_background_effect_manager_v1_interface.name) == 0) {
        state->manager_name = name;
        state->manager_version = version;
    } else if (strcmp(interface, wl_compositor_interface.name) == 0) {
        state->compositor_name = name;
        state->compositor_version = version;
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry,
                                   uint32_t name) {
    (void) data;
    (void) registry;
    (void) name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

static void manager_capabilities(void *data,
                                 struct ext_background_effect_manager_v1 *manager,
                                 uint32_t flags) {
    (void) manager;
    struct capability_state *state = data;
    state->flags = flags;
}

static const struct ext_background_effect_manager_v1_listener manager_listener = {
    .capabilities = manager_capabilities,
};

static void add_rounded_rect(struct wl_region *region, int x, int y,
                             int width, int height, int radius) {
    if (radius < 0) radius = 0;
    if (radius * 2 > width) radius = width / 2;
    if (radius * 2 > height) radius = height / 2;

    for (int row = 0; row < height; row++) {
        int dy = 0;
        if (row < radius) {
            dy = radius - row;
        } else if (row >= height - radius) {
            dy = row - (height - radius - 1);
        }

        int inset = 0;
        while (inset < radius) {
            int dx = radius - inset;
            if (dx * dx + dy * dy <= radius * radius) break;
            inset++;
        }
        if (width > inset * 2) {
            wl_region_add(region, x + inset, y + row, width - inset * 2, 1);
        }
    }
}

/* Pixel rectangle of `widget` in the native's *surface* coordinates, rounded
 * inwards so the blur can never poke out past the painted shape (rounding
 * outwards left a visible blurred sliver beyond the rounded corners). Bounds
 * relative to the native are already in surface coordinates here; do NOT add
 * gtk_native_get_surface_transform, that double-counts the popover's shadow
 * margin and the window's frame offset and shifts the blur. */
static int widget_surface_rect(GtkNative *native, GtkWidget *widget,
                               int surface_width, int surface_height,
                               int *out_x, int *out_y, int *out_w, int *out_h) {
    graphene_rect_t bounds;
    if (!gtk_widget_compute_bounds(widget, GTK_WIDGET(native), &bounds)) return 0;

    int x0 = (int) ceilf(bounds.origin.x - 0.01f);
    int y0 = (int) ceilf(bounds.origin.y - 0.01f);
    int x1 = (int) floorf(bounds.origin.x + bounds.size.width + 0.01f);
    int y1 = (int) floorf(bounds.origin.y + bounds.size.height + 0.01f);
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > surface_width) x1 = surface_width;
    if (y1 > surface_height) y1 = surface_height;
    if (x1 <= x0 || y1 <= y0) return 0;

    *out_x = x0;
    *out_y = y0;
    *out_w = x1 - x0;
    *out_h = y1 - y0;
    return 1;
}

static int set_blur_region(struct blur_handle *handle, GtkNative *native,
                           GtkWidget *region_widget, int corner_radius) {
    GdkSurface *gdk_surface = gtk_native_get_surface(native);
    int width = gdk_surface ? gdk_surface_get_width(gdk_surface) : 0;
    int height = gdk_surface ? gdk_surface_get_height(gdk_surface) : 0;
    if (!gdk_surface || width <= 0 || height <= 0) return 0;

    struct wl_region *region = wl_compositor_create_region(handle->compositor);
    if (!region) return 0;

    int x = 0;
    int y = 0;
    int region_width = width;
    int region_height = height;
    if (region_widget &&
        !widget_surface_rect(native, region_widget, width, height,
                             &x, &y, &region_width, &region_height)) {
        wl_region_destroy(region);
        return 0;
    }

    add_rounded_rect(region, x, y, region_width, region_height, corner_radius);
    ext_background_effect_surface_v1_set_blur_region(handle->effect, region);
    wl_region_destroy(region);
    wl_surface_commit(handle->surface);
    if (wl_display_flush(handle->display) < 0) return 0;
    return 1;
}

/* Blurs the union of several rounded widget rectangles (and nothing else). */
static int set_blur_regions(struct blur_handle *handle, GtkNative *native,
                            GtkWidget **widgets, const int *radii, int count) {
    GdkSurface *gdk_surface = gtk_native_get_surface(native);
    int width = gdk_surface ? gdk_surface_get_width(gdk_surface) : 0;
    int height = gdk_surface ? gdk_surface_get_height(gdk_surface) : 0;
    if (!gdk_surface || width <= 0 || height <= 0) return 0;

    struct wl_region *region = wl_compositor_create_region(handle->compositor);
    if (!region) return 0;

    for (int i = 0; i < count; i++) {
        int x, y, w, h;
        if (!widgets[i] || !gtk_widget_get_mapped(widgets[i]) ||
            !widget_surface_rect(native, widgets[i], width, height, &x, &y, &w, &h)) {
            continue;
        }
        add_rounded_rect(region, x, y, w, h, radii[i]);
    }

    ext_background_effect_surface_v1_set_blur_region(handle->effect, region);
    wl_region_destroy(region);
    wl_surface_commit(handle->surface);
    if (wl_display_flush(handle->display) < 0) return 0;
    return 1;
}

static void destroy_discovery(struct wl_event_queue *queue,
                              struct wl_registry *registry,
                              struct ext_background_effect_manager_v1 *manager,
                              struct wl_compositor *compositor,
                              struct ext_background_effect_surface_v1 *effect) {
    if (effect) ext_background_effect_surface_v1_destroy(effect);
    if (manager) ext_background_effect_manager_v1_destroy(manager);
    if (compositor) wl_compositor_destroy(compositor);
    if (registry) wl_registry_destroy(registry);
    if (queue) wl_event_queue_destroy(queue);
}

static void *enable_native_blur(GtkNative *native, GtkWidget *region_widget,
                                int corner_radius, const char *surface_name) {
    if (!native) return NULL;

    GdkSurface *gdk_surface = gtk_native_get_surface(native);
    if (!gdk_surface ||
        !GDK_IS_WAYLAND_DISPLAY(gdk_surface_get_display(gdk_surface))) {
        report_blur_unavailable("GTK surface is not a Wayland surface");
        return NULL;
    }

    struct wl_display *display = gdk_wayland_display_get_wl_display(
        gdk_surface_get_display(gdk_surface));
    struct wl_surface *surface = gdk_wayland_surface_get_wl_surface(gdk_surface);
    if (!display || !surface) {
        report_blur_unavailable("GTK surface has no Wayland handle");
        return NULL;
    }

    struct wl_event_queue *queue = wl_display_create_queue(display);
    struct wl_registry *registry = wl_display_get_registry(display);
    if (!queue || !registry) {
        report_blur_unavailable("could not create a Wayland registry queue");
        if (registry) wl_registry_destroy(registry);
        if (queue) wl_event_queue_destroy(queue);
        return NULL;
    }
    wl_proxy_set_queue((struct wl_proxy *) registry, queue);

    struct registry_state globals = {0};
    wl_registry_add_listener(registry, &registry_listener, &globals);
    if (wl_display_roundtrip_queue(display, queue) < 0) {
        report_protocol_error(display, "registry discovery");
        destroy_discovery(queue, registry, NULL, NULL, NULL);
        return NULL;
    }
    if (!globals.manager_name || !globals.compositor_name) {
        report_blur_unavailable("compositor does not expose ext-background-effect-v1");
        destroy_discovery(queue, registry, NULL, NULL, NULL);
        return NULL;
    }

    uint32_t manager_version = globals.manager_version < 1 ? globals.manager_version : 1;
    uint32_t compositor_version = globals.compositor_version < 1 ? globals.compositor_version : 1;
    struct ext_background_effect_manager_v1 *manager =
        wl_registry_bind(registry, globals.manager_name,
                         &ext_background_effect_manager_v1_interface,
                         manager_version);
    struct wl_compositor *compositor =
        wl_registry_bind(registry, globals.compositor_name,
                         &wl_compositor_interface, compositor_version);
    if (!manager || !compositor) {
        report_blur_unavailable("could not bind the compositor blur interfaces");
        destroy_discovery(queue, registry, manager, compositor, NULL);
        return NULL;
    }
    wl_proxy_set_queue((struct wl_proxy *) manager, queue);
    wl_proxy_set_queue((struct wl_proxy *) compositor, queue);

    struct capability_state capabilities = {0};
    if (ext_background_effect_manager_v1_add_listener(manager, &manager_listener,
                                                        &capabilities) < 0 ||
        wl_display_roundtrip_queue(display, queue) < 0) {
        report_protocol_error(display, "blur capability query");
        destroy_discovery(queue, registry, manager, compositor, NULL);
        return NULL;
    }
    if (!(capabilities.flags & EXT_BACKGROUND_EFFECT_MANAGER_V1_CAPABILITY_BLUR)) {
        report_blur_unavailable("KWin did not advertise the blur capability");
        destroy_discovery(queue, registry, manager, compositor, NULL);
        return NULL;
    }

    struct ext_background_effect_surface_v1 *effect =
        ext_background_effect_manager_v1_get_background_effect(manager, surface);
    if (!effect) {
        report_blur_unavailable("KWin refused a background-effect object");
        destroy_discovery(queue, registry, manager, compositor, NULL);
        return NULL;
    }
    wl_proxy_set_queue((struct wl_proxy *) effect, queue);

    struct blur_handle *handle = malloc(sizeof(*handle));
    if (!handle) {
        report_blur_unavailable("could not allocate the blur handle");
        destroy_discovery(queue, registry, manager, compositor, effect);
        wl_surface_commit(surface);
        wl_display_flush(display);
        return NULL;
    }
    handle->effect = effect;
    handle->surface = surface;
    handle->compositor = compositor;
    handle->display = display;

    if (!set_blur_region(handle, native, region_widget, corner_radius)) {
        report_protocol_error(display, "setting the blur region");
        wl_proxy_set_queue((struct wl_proxy *) effect, NULL);
        wl_proxy_set_queue((struct wl_proxy *) compositor, NULL);
        destroy_discovery(queue, registry, manager, compositor, effect);
        wl_display_flush(display);
        free(handle);
        return NULL;
    }

    if (wl_display_roundtrip_queue(display, queue) < 0) {
        report_protocol_error(display, "initial blur-region commit");
        destroy_discovery(queue, registry, manager, compositor, effect);
        wl_display_flush(display);
        free(handle);
        return NULL;
    }

    /* Keep the effect and compositor alive, but return their event queues to
     * GDK's default queue before releasing the temporary discovery queue. */
    wl_proxy_set_queue((struct wl_proxy *) effect, NULL);
    wl_proxy_set_queue((struct wl_proxy *) compositor, NULL);
    destroy_discovery(queue, registry, manager, NULL, NULL);
    wl_display_flush(display);

    fprintf(stderr, "lunar-desktop: KWin backdrop blur attached to %s\n",
            surface_name);
    return handle;
}

void *lunar_kde_blur_enable(GtkPopover *popover) {
    return popover ? enable_native_blur(GTK_NATIVE(popover), GTK_WIDGET(popover),
                                        12, "popover") : NULL;
}

void *lunar_kde_blur_enable_popover_region(GtkPopover *popover,
                                            GtkWidget *region_widget,
                                            int corner_radius) {
    return popover ? enable_native_blur(GTK_NATIVE(popover),
                                        region_widget ? region_widget : GTK_WIDGET(popover),
                                        corner_radius, "popover") : NULL;
}

int lunar_kde_blur_update_popover_region(void *opaque, GtkPopover *popover,
                                          GtkWidget *region_widget,
                                          int corner_radius) {
    struct blur_handle *handle = opaque;
    if (!handle || !popover) return 0;
    return set_blur_region(handle, GTK_NATIVE(popover),
                           region_widget ? region_widget : GTK_WIDGET(popover),
                           corner_radius);
}

int lunar_kde_blur_update_popover_regions(void *opaque, GtkPopover *popover,
                                           GtkWidget **widgets, const int *radii,
                                           int count) {
    struct blur_handle *handle = opaque;
    if (!handle || !popover || !widgets || !radii || count <= 0) return 0;
    return set_blur_regions(handle, GTK_NATIVE(popover), widgets, radii, count);
}

void *lunar_kde_blur_enable_widget(GtkWindow *window, GtkWidget *region_widget,
                                   int corner_radius) {
    return window && region_widget
        ? enable_native_blur(GTK_NATIVE(window), region_widget, corner_radius,
                             "widget region")
        : NULL;
}

int lunar_kde_blur_update_widget(void *opaque, GtkWindow *window,
                                  GtkWidget *region_widget, int corner_radius) {
    struct blur_handle *handle = opaque;
    if (!handle || !window || !region_widget) return 0;
    return set_blur_region(handle, GTK_NATIVE(window), region_widget, corner_radius);
}

int lunar_kde_blur_update_widgets(void *opaque, GtkWindow *window,
                                   GtkWidget **widgets, const int *radii,
                                   int count) {
    struct blur_handle *handle = opaque;
    if (!handle || !window || !widgets || !radii || count <= 0) return 0;
    return set_blur_regions(handle, GTK_NATIVE(window), widgets, radii, count);
}

void lunar_kde_blur_disable(void *opaque) {
    struct blur_handle *handle = opaque;
    if (!handle) return;
    ext_background_effect_surface_v1_destroy(handle->effect);
    wl_display_flush(handle->display);
    wl_compositor_destroy(handle->compositor);
    wl_display_flush(handle->display);
    free(handle);
}

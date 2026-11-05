//! Reusable glass context-menu construction for shell surfaces.

use std::{cell::RefCell, ffi::c_void, ptr, rc::Rc};

use gtk::prelude::*;

unsafe extern "C" {
    fn lunar_kde_blur_enable_popover_region(
        popover: *mut gtk::ffi::GtkPopover,
        region_widget: *mut gtk::ffi::GtkWidget,
        corner_radius: i32,
    ) -> *mut c_void;
    fn lunar_kde_blur_update_popover_region(
        handle: *mut c_void,
        popover: *mut gtk::ffi::GtkPopover,
        region_widget: *mut gtk::ffi::GtkWidget,
        corner_radius: i32,
    ) -> i32;
    fn lunar_kde_blur_update_popover_regions(
        handle: *mut c_void,
        popover: *mut gtk::ffi::GtkPopover,
        widgets: *mut *mut gtk::ffi::GtkWidget,
        radii: *const i32,
        count: i32,
    ) -> i32;
    fn lunar_kde_blur_enable_widget(
        window: *mut gtk::ffi::GtkWindow,
        region_widget: *mut gtk::ffi::GtkWidget,
        corner_radius: i32,
    ) -> *mut c_void;
    fn lunar_kde_blur_update_widget(
        handle: *mut c_void,
        window: *mut gtk::ffi::GtkWindow,
        region_widget: *mut gtk::ffi::GtkWidget,
        corner_radius: i32,
    ) -> i32;
    fn lunar_kde_blur_disable(handle: *mut c_void);
}

/// A blue glass popover menu that can be attached to any GTK widget.
#[derive(Clone)]
pub struct ContextMenu {
    popover: gtk::Popover,
    content: gtk::Box,
}

impl ContextMenu {
    /// Creates a menu using the shared shell-menu style and extra caller styles.
    pub fn new(position: gtk::PositionType, style_classes: &[&str]) -> Self {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        popover.set_position(position);
        popover.add_css_class("lunar-menu-popover");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("lunar-system-menu");
        for class in style_classes {
            content.add_css_class(class);
        }
        popover.set_child(Some(&content));
        crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
        // Blur only the menu body (not the arrow gap around it).
        attach_blur_region(
            &popover,
            content.upcast_ref::<gtk::Widget>(),
            crate::liquid_glass::Material::menu().blur_corner(),
        );

        Self { popover, content }
    }

    pub fn set_parent(&self, parent: &impl IsA<gtk::Widget>) {
        self.popover.set_parent(parent);
    }

    /// Shows or hides the little arrow that points at the widget the menu is
    /// attached to.
    pub fn set_arrow(&self, has_arrow: bool) {
        self.popover.set_has_arrow(has_arrow);
    }

    /// Adds a CSS class to the popover itself (e.g. to style its arrow).
    pub fn add_popover_class(&self, class: &str) {
        self.popover.add_css_class(class);
    }

    /// Opens the menu pointing at the whole widget it is attached to, so the
    /// arrow sits centred on it.
    pub fn popup_on_parent(&self) {
        self.popover.set_pointing_to(None::<&gtk::gdk::Rectangle>);
        self.popover.popup();
    }

    #[allow(dead_code)]
    pub fn popup_at(&self, x: i32, y: i32) {
        let rect = gtk::gdk::Rectangle::new(x, y, 1, 1);
        self.popover.set_pointing_to(Some(&rect));
        self.popover.popup();
    }

    pub fn popdown(&self) {
        self.popover.popdown();
    }

    /// Detaches the menu from its parent; call when the parent is destroyed.
    pub fn unparent(&self) {
        self.popover.unparent();
    }

    pub fn append_action(
        &self,
        title: &str,
        enabled: bool,
        action: impl Fn() + 'static,
    ) -> gtk::Button {
        let button = menu_button(title, None);
        button.set_sensitive(enabled);
        let popover = self.popover.downgrade();
        button.connect_clicked(move |_| {
            action();
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });
        self.content.append(&button);
        button
    }

    pub fn append_separator(&self) {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.add_css_class("lunar-menu-separator");
        separator.add_css_class("lunar-context-menu-separator");
        self.content.append(&separator);
    }

    /// Adds a right-opening submenu that opens on hover or click.
    pub fn append_submenu(
        &self,
        title: &str,
        style_classes: &[&str],
        build: impl FnOnce(&ContextMenu),
    ) -> gtk::Button {
        let button = menu_button(title, Some("›"));
        self.content.append(&button);

        let submenu = ContextMenu::new(gtk::PositionType::Right, style_classes);
        build(&submenu);
        submenu.set_parent(&button);

        let submenu_for_click = submenu.popover.clone();
        button.connect_clicked(move |_| submenu_for_click.popup());
        let motion = gtk::EventControllerMotion::new();
        let submenu_for_hover = submenu.popover.clone();
        motion.connect_enter(move |_, _, _| submenu_for_hover.popup());
        button.add_controller(motion);

        let submenu_for_close = submenu.popover.downgrade();
        self.popover.connect_closed(move |_| {
            if let Some(submenu) = submenu_for_close.upgrade() {
                submenu.popdown();
            }
        });
        button
    }
}

fn menu_button(title: &str, trailing: Option<&str>) -> gtk::Button {
    let button = gtk::Button::new();
    let contents = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let text = gtk::Label::new(Some(title));
    text.set_halign(gtk::Align::Start);
    text.set_hexpand(true);
    contents.append(&text);
    if let Some(trailing) = trailing {
        let arrow = gtk::Label::new(Some(trailing));
        arrow.add_css_class("lunar-context-menu-arrow");
        contents.append(&arrow);
    }
    button.set_child(Some(&contents));
    button.add_css_class("lunar-menu-item");
    button.add_css_class("lunar-context-menu-item");
    button.set_halign(gtk::Align::Fill);
    button
}

/// Attaches the same KWin compositor blur to any compatible GTK popover.
pub fn attach_blur(popover: &gtk::Popover) {
    let handle: Rc<RefCell<*mut c_void>> = Rc::new(RefCell::new(ptr::null_mut()));
    let on_map = Rc::clone(&handle);
    popover.connect_map(move |popover| {
        let weak_popover = popover.downgrade();
        let on_map = Rc::clone(&on_map);
        // Let GTK finish mapping and committing the popup surface before
        // attaching the compositor effect to it.
        gtk::glib::idle_add_local_once(move || {
            let Some(popover) = weak_popover.upgrade() else {
                return;
            };
            let needs_blur = on_map.borrow().is_null();
            if popover.is_mapped() && needs_blur {
                // Same call as `lunar_kde_blur_enable`, but the corner radius is the
                // menu material's instead of a number baked into the C side.
                let radius = crate::liquid_glass::Material::menu().blur_corner();
                let blur = unsafe {
                    lunar_kde_blur_enable_popover_region(popover.as_ptr(), ptr::null_mut(), radius)
                };
                *on_map.borrow_mut() = blur;
            }
        });
    });
    // A popover is unmapped every time it is dismissed, but GTK keeps its
    // native popup surface around for the next opening. Keep the compositor
    // effect attached across those map/unmap cycles; destroying and recreating
    // it on every click races GTK's CSS/surface state updates.
    let on_unrealize = Rc::clone(&handle);
    popover.connect_unrealize(move |_| {
        let handle = std::mem::replace(&mut *on_unrealize.borrow_mut(), ptr::null_mut());
        if !handle.is_null() {
            unsafe { lunar_kde_blur_disable(handle) };
        }
    });
}

/// Like `attach_blur`, but blurs only `region` (a child of the popover) instead
/// of the whole popover surface, which matters when the popover has an arrow.
pub fn attach_blur_region(
    popover: &gtk::Popover,
    region: &gtk::Widget,
    corner_radius: i32,
) -> Rc<dyn Fn()> {
    let handle: Rc<RefCell<*mut c_void>> = Rc::new(RefCell::new(ptr::null_mut()));
    let weak_region = region.downgrade();
    let on_map = Rc::clone(&handle);
    popover.connect_map(move |popover| {
        let weak_popover = popover.downgrade();
        let weak_region = weak_region.clone();
        let on_map = Rc::clone(&on_map);
        gtk::glib::idle_add_local_once(move || {
            let (Some(popover), Some(region)) = (weak_popover.upgrade(), weak_region.upgrade())
            else {
                return;
            };
            if !popover.is_mapped() {
                return;
            }
            let mut active = on_map.borrow_mut();
            if active.is_null() {
                *active = unsafe {
                    lunar_kde_blur_enable_popover_region(
                        popover.as_ptr(),
                        region.as_ptr(),
                        corner_radius,
                    )
                };
            } else {
                // The popup surface is reused; refresh the region each time.
                unsafe {
                    lunar_kde_blur_update_popover_region(
                        *active,
                        popover.as_ptr(),
                        region.as_ptr(),
                        corner_radius,
                    );
                }
            }
        });
    });
    let on_unrealize = Rc::clone(&handle);
    popover.connect_unrealize(move |_| {
        let handle = std::mem::replace(&mut *on_unrealize.borrow_mut(), ptr::null_mut());
        if !handle.is_null() {
            unsafe { lunar_kde_blur_disable(handle) };
        }
    });

    // Re-fits the blur to the region after it changes size (e.g. a section
    // expanding inside the open popover).
    let refresh_popover = popover.downgrade();
    let refresh_region = region.downgrade();
    Rc::new(move || {
        for delay in [30u64, 220] {
            let weak_popover = refresh_popover.clone();
            let weak_region = refresh_region.clone();
            let handle = Rc::clone(&handle);
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                let (Some(popover), Some(region)) = (weak_popover.upgrade(), weak_region.upgrade())
                else {
                    return;
                };
                let active = *handle.borrow();
                if popover.is_mapped() && !active.is_null() {
                    unsafe {
                        lunar_kde_blur_update_popover_region(
                            active,
                            popover.as_ptr(),
                            region.as_ptr(),
                            corner_radius,
                        );
                    }
                }
            });
        }
    })
}

/// Blurs only the given widgets (each with its own corner radius) and nothing
/// between or around them. Returns a callback that re-fits the blur.
pub fn attach_blur_regions(
    popover: &gtk::Popover,
    regions: Vec<(gtk::Widget, i32)>,
) -> Rc<dyn Fn()> {
    let regions = Rc::new(regions);
    let handle: Rc<RefCell<*mut c_void>> = Rc::new(RefCell::new(ptr::null_mut()));

    let apply = {
        let regions = Rc::clone(&regions);
        let handle = Rc::clone(&handle);
        move |popover: &gtk::Popover| {
            let mut widgets: Vec<*mut gtk::ffi::GtkWidget> = regions
                .iter()
                .map(|(widget, _)| widget.as_ptr())
                .collect();
            let radii: Vec<i32> = regions.iter().map(|(_, radius)| *radius).collect();
            let mut active = handle.borrow_mut();
            if active.is_null() {
                // The first region creates the effect; all of them are applied below.
                let Some((first, radius)) = regions.first() else {
                    return;
                };
                *active = unsafe {
                    lunar_kde_blur_enable_popover_region(popover.as_ptr(), first.as_ptr(), *radius)
                };
            }
            if !active.is_null() {
                unsafe {
                    lunar_kde_blur_update_popover_regions(
                        *active,
                        popover.as_ptr(),
                        widgets.as_mut_ptr(),
                        radii.as_ptr(),
                        widgets.len() as i32,
                    );
                }
            }
        }
    };
    let apply = Rc::new(apply);

    let on_map = Rc::clone(&apply);
    popover.connect_map(move |popover| {
        let weak_popover = popover.downgrade();
        let on_map = Rc::clone(&on_map);
        gtk::glib::idle_add_local_once(move || {
            let Some(popover) = weak_popover.upgrade() else {
                return;
            };
            if popover.is_mapped() {
                on_map(&popover);
            }
        });
    });
    let on_unrealize = Rc::clone(&handle);
    popover.connect_unrealize(move |_| {
        let handle = std::mem::replace(&mut *on_unrealize.borrow_mut(), ptr::null_mut());
        if !handle.is_null() {
            unsafe { lunar_kde_blur_disable(handle) };
        }
    });

    let refresh_popover = popover.downgrade();
    Rc::new(move || {
        let weak_popover = refresh_popover.clone();
        let apply = Rc::clone(&apply);
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
            if let Some(popover) = weak_popover.upgrade() {
                if popover.is_mapped() {
                    apply(&popover);
                }
            }
        });
    })
}

/// Attaches the compositor blur to a whole native GTK window.
pub fn attach_window_blur(application_window: &gtk::ApplicationWindow) {
    let window: &gtk::Window = application_window.upcast_ref();
    let _refresh = attach_window_widget_blur(window, window, 12);
}

/// Attaches compositor blur to a specific rounded widget region within a GTK
/// window. The returned callback refreshes the region after the window resizes.
pub fn attach_window_widget_blur(
    window: &gtk::Window,
    region_widget: &impl IsA<gtk::Widget>,
    corner_radius: i32,
) -> Rc<dyn Fn()> {
    let handle: Rc<RefCell<*mut c_void>> = Rc::new(RefCell::new(ptr::null_mut()));
    let mapped = Rc::new(std::cell::Cell::new(false));
    let resize_signals_connected = Rc::new(std::cell::Cell::new(false));
    let weak_window = window.downgrade();
    let weak_region = region_widget.as_ref().downgrade();
    let refresh: Rc<dyn Fn()> = {
        let handle = Rc::clone(&handle);
        let mapped = Rc::clone(&mapped);
        Rc::new(move || {
            if !mapped.get() {
                return;
            }
            if let (Some(window), Some(region)) = (weak_window.upgrade(), weak_region.upgrade()) {
                let mut active_handle = handle.borrow_mut();
                if active_handle.is_null() {
                    *active_handle = unsafe {
                        lunar_kde_blur_enable_widget(
                            window.as_ptr(),
                            region.as_ptr(),
                            corner_radius,
                        )
                    };
                } else {
                    let updated = unsafe {
                        lunar_kde_blur_update_widget(
                            *active_handle,
                            window.as_ptr(),
                            region.as_ptr(),
                            corner_radius,
                        )
                    };
                    if updated == 0 {
                        eprintln!("lunar-desktop: KWin backdrop blur region update failed");
                    }
                }
            }
        })
    };

    let map_refresh = Rc::clone(&refresh);
    let map_state = Rc::clone(&mapped);
    let signals_connected = Rc::clone(&resize_signals_connected);
    window.connect_map(move |window| {
        map_state.set(true);
        // Wait until GTK has allocated the native surface and committed its
        // first size; an earlier request can produce an empty blur region.
        let initial_refresh = Rc::clone(&map_refresh);
        gtk::glib::idle_add_local_once(move || initial_refresh());
        if !signals_connected.replace(true) {
            if let Some(surface) = window.surface() {
                let pending = Rc::new(std::cell::Cell::new(false));
                let schedule: Rc<dyn Fn()> = {
                    let refresh = Rc::clone(&map_refresh);
                    Rc::new(move || {
                        if pending.replace(true) {
                            return;
                        }
                        let pending = Rc::clone(&pending);
                        let refresh = Rc::clone(&refresh);
                        gtk::glib::idle_add_local_once(move || {
                            pending.set(false);
                            refresh();
                        });
                    })
                };
                let width_schedule = Rc::clone(&schedule);
                surface.connect_width_notify(move |_| width_schedule());
                let height_schedule = Rc::clone(&schedule);
                surface.connect_height_notify(move |_| height_schedule());
            }
        }
    });
    let unmap_handle = Rc::clone(&handle);
    let unmap_state = Rc::clone(&mapped);
    window.connect_unmap(move |_| {
        unmap_state.set(false);
        let handle = std::mem::replace(&mut *unmap_handle.borrow_mut(), ptr::null_mut());
        if !handle.is_null() {
            unsafe { lunar_kde_blur_disable(handle) };
        }
    });
    refresh
}

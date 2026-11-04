//! Places the wallpaper and dock surfaces through KWin's Plasma shell protocol.

use std::{cell::RefCell, ffi::c_void, ptr, rc::Rc};

use gtk::{gdk, prelude::*};

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum Role {
    Desktop = 1,
    Panel = 2,
}

unsafe extern "C" {
    fn lunar_kde_shell_is_supported(display: *mut gdk::ffi::GdkDisplay) -> i32;
    fn lunar_kde_shell_attach(
        window: *mut gtk::ffi::GtkWindow,
        monitor: *mut gdk::ffi::GdkMonitor,
        role: u32,
        x: i32,
        y: i32,
    ) -> *mut c_void;
    fn lunar_kde_shell_detach(handle: *mut c_void);
    fn lunar_kde_shell_set_position(handle: *mut c_void, x: i32, y: i32);
}

pub type PositionUpdater = Rc<dyn Fn(i32, i32)>;

pub fn is_supported() -> bool {
    gdk::Display::default()
        .is_some_and(|display| unsafe { lunar_kde_shell_is_supported(display.as_ptr()) != 0 })
}

/// Gives a GTK window its desktop or panel role and output position.
pub fn configure_window(
    window: &gtk::Window,
    monitor: &gdk::Monitor,
    role: Role,
    x: i32,
    y: i32,
) -> PositionUpdater {
    let monitor = monitor.clone();
    let handle: Rc<RefCell<*mut c_void>> = Rc::new(RefCell::new(ptr::null_mut()));

    let on_realize = Rc::clone(&handle);
    let role = role as u32;
    window.connect_realize(move |window| {
        if on_realize.borrow().is_null() {
            *on_realize.borrow_mut() =
                unsafe { lunar_kde_shell_attach(window.as_ptr(), monitor.as_ptr(), role, x, y) };
        }
    });

    let on_unrealize = Rc::clone(&handle);
    window.connect_unrealize(move |_| {
        let handle = std::mem::replace(&mut *on_unrealize.borrow_mut(), ptr::null_mut());
        if !handle.is_null() {
            unsafe { lunar_kde_shell_detach(handle) };
        }
    });

    let updater_handle = Rc::clone(&handle);
    Rc::new(move |x, y| {
        let handle = *updater_handle.borrow();
        if !handle.is_null() {
            unsafe { lunar_kde_shell_set_position(handle, x, y) };
        }
    })
}

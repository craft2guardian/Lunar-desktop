//! Real window thumbnails for the Dock's minimised-window tiles.
//!
//! KWin's `org.kde.KWin.ScreenShot2.CaptureWindow` writes a window's pixels
//! into a pipe. A minimised window is no longer drawn, so it cannot be captured
//! at the moment it is minimised; instead the Dock keeps refreshing a small
//! thumbnail of every visible window and shows the latest one when the window
//! is minimised.
//!
//! KWin only answers apps that declare the interface in their desktop file
//! (see `scripts/install-thumbnail-permission.sh`). Without that, capturing
//! fails and the Dock keeps showing its placeholder card.
//!
//! Capturing and shrinking run on worker threads; only the finished, small
//! pixel buffers come back to the GTK thread, where they become textures.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Thumbnails are shrunk to fit inside this many pixels.
const MAX_WIDTH: usize = 360;
const MAX_HEIGHT: usize = 260;
/// Never keep more than this many captures running at once.
const MAX_IN_FLIGHT: usize = 1;

/// Only the first few capture attempts are traced, to keep the log readable.
static TRACED: AtomicU32 = AtomicU32::new(0);

fn trace(message: impl FnOnce() -> String) {
    if TRACED.fetch_add(1, Ordering::Relaxed) < 40 {
        eprintln!("[thumbnail] {}", message());
    }
}

struct Frame {
    window_id: String,
    width: usize,
    height: usize,
    format: gdk::MemoryFormat,
    data: Vec<u8>,
}

pub struct Thumbnailer {
    sender: Sender<(String, Result<Frame, String>)>,
    receiver: Receiver<(String, Result<Frame, String>)>,
    in_flight: RefCell<HashSet<String>>,
    textures: RefCell<HashMap<String, gdk::Texture>>,
    /// Distinct failure messages already printed, so the log is not flooded.
    reported: RefCell<HashSet<String>>,
    reported_success: Cell<bool>,
}

impl Thumbnailer {
    pub fn new() -> Self {
        static PERMISSION: std::sync::Once = std::sync::Once::new();
        PERMISSION.call_once(|| {
            std::thread::spawn(ensure_permission);
        });
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            in_flight: RefCell::new(HashSet::new()),
            textures: RefCell::new(HashMap::new()),
            reported: RefCell::new(HashSet::new()),
            reported_success: Cell::new(false),
        }
    }

    pub fn texture(&self, window_id: &str) -> Option<gdk::Texture> {
        self.textures.borrow().get(window_id).cloned()
    }

    pub fn has_texture(&self, window_id: &str) -> bool {
        self.textures.borrow().contains_key(window_id)
    }

    /// Starts capturing `window_id` in the background, unless it already is.
    pub fn request(&self, window_id: &str) {
        let mut in_flight = self.in_flight.borrow_mut();
        if in_flight.len() >= MAX_IN_FLIGHT || in_flight.contains(window_id) {
            return;
        }
        in_flight.insert(window_id.to_string());
        let id = window_id.to_string();
        let sender = self.sender.clone();
        trace(|| format!("capturing {window_id}"));
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = capture(&id);
            trace(|| match &result {
                Ok(frame) => format!(
                    "{id}: ok {}x{} in {} ms",
                    frame.width,
                    frame.height,
                    started.elapsed().as_millis()
                ),
                Err(error) => format!("{id}: failed after {} ms: {error}", started.elapsed().as_millis()),
            });
            let _ = sender.send((id, result));
        });
    }

    /// Turns finished captures into textures; returns the windows that got a
    /// new one. A capture that finishes after its window was minimised can come
    /// back empty, so for `minimised` windows that already have a thumbnail the
    /// existing one is kept.
    pub fn poll(&self, minimised: &HashSet<String>) -> Vec<String> {
        let mut updated = Vec::new();
        while let Ok((id, result)) = self.receiver.try_recv() {
            self.in_flight.borrow_mut().remove(&id);
            match result {
                Ok(frame) => {
                    if minimised.contains(&frame.window_id)
                        && self.textures.borrow().contains_key(&frame.window_id)
                    {
                        continue;
                    }
                    if !self.reported_success.replace(true) {
                        eprintln!(
                            "lunar-desktop: thumbnail captured for {} ({}x{})",
                            frame.window_id, frame.width, frame.height
                        );
                    }
                    let bytes = glib::Bytes::from_owned(frame.data);
                    let texture = gdk::MemoryTexture::new(
                        frame.width as i32,
                        frame.height as i32,
                        frame.format,
                        &bytes,
                        frame.width * 4,
                    );
                    self.textures
                        .borrow_mut()
                        .insert(frame.window_id.clone(), texture.upcast());
                    updated.push(frame.window_id);
                }
                Err(error) => {
                    // Each distinct reason is printed once.
                    if self.reported.borrow_mut().insert(error.clone()) {
                        eprintln!(
                            "lunar-desktop: thumbnail capture of {id} failed: {error} \
                             (if this is a permission error, run \
                             scripts/install-thumbnail-permission.sh with the path of the \
                             binary you run, then restart the shell)"
                        );
                    }
                }
            }
        }
        if !updated.is_empty() {
            release_freed_memory();
        }
        updated
    }

    /// Drops thumbnails of windows that no longer exist.
    pub fn retain(&self, live: &HashSet<&str>) {
        self.textures
            .borrow_mut()
            .retain(|id, _| live.contains(id.as_str()));
    }
}

/// Hands memory freed by the (large, short-lived) screenshot buffers back to the OS.
pub(crate) fn release_freed_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: malloc_trim only returns unused heap pages to the OS.
        unsafe {
            malloc_trim(0);
        }
    }
}

/// KWin lets a program take screenshots only if a desktop file whose `Exec`
/// is that program's executable lists `org.kde.KWin.ScreenShot2` under
/// `X-KDE-DBUS-Restricted-Interfaces`. Writing that file for the running
/// executable here means it matches however the shell was started (cargo run,
/// target/debug, target/release, an installed copy).
fn ensure_permission() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("[thumbnail] cannot find own executable: {error}");
            return;
        }
    };
    let applications = glib::user_data_dir().join("applications");
    let path = applications.join("dev.lunar.Desktop.desktop");
    let exec = executable.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=Lunar Desktop\nExec=\"{exec}\"\n\
         NoDisplay=true\nX-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n"
    );
    let unchanged = std::fs::read_to_string(&path).is_ok_and(|current| current == contents);
    eprintln!(
        "[thumbnail] executable {}; permission file {} ({})",
        executable.display(),
        path.display(),
        if unchanged { "already up to date" } else { "updating" }
    );
    if !unchanged {
        let written = std::fs::create_dir_all(&applications)
            .and_then(|_| std::fs::write(&path, contents));
        if let Err(error) = written {
            eprintln!("[thumbnail] could not write {}: {error}", path.display());
            return;
        }
        // KWin finds the file through the KDE service cache.
        for tool in ["kbuildsycoca6", "kbuildsycoca5"] {
            match std::process::Command::new(tool).output() {
                Ok(_) => {
                    eprintln!("[thumbnail] refreshed the KDE service cache with {tool}");
                    break;
                }
                Err(_) => continue,
            }
        }
    }

    // KWin uses the first desktop file that launches this executable, so
    // another one would hide ours.
    let mut directories = vec![glib::user_data_dir()];
    directories.extend(glib::system_data_dirs());
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory.join("applications")) else {
            continue;
        };
        for entry in entries.flatten() {
            let other = entry.path();
            if other == path || other.extension().is_none_or(|ext| ext != "desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&other) else {
                continue;
            };
            let launches_us = text
                .lines()
                .find_map(|line| line.strip_prefix("Exec="))
                .and_then(|exec| exec.split_whitespace().next())
                .map(|program| program.trim_matches('"'))
                .is_some_and(|program| std::fs::canonicalize(program).is_ok_and(|p| p == executable));
            if launches_us {
                eprintln!(
                    "[thumbnail] warning: {} also launches this executable and may take \
                     priority over {}",
                    other.display(),
                    path.display()
                );
            }
        }
    }
}

/// Qt's `QImage::Format` values that KWin may report, as GDK formats. KWin
/// writes native-endian 32-bit pixels, which on little-endian machines is the
/// byte order GDK names below.
///
/// The second value is true for formats whose fourth byte is unused padding
/// rather than alpha; the caller makes it opaque.
fn gdk_format(qt_format: u32) -> Option<(gdk::MemoryFormat, bool)> {
    use gdk::MemoryFormat as F;
    Some(match qt_format {
        4 => (F::B8g8r8a8Premultiplied, true),  // Format_RGB32
        5 => (F::B8g8r8a8, false),              // Format_ARGB32
        6 => (F::B8g8r8a8Premultiplied, false), // Format_ARGB32_Premultiplied
        16 => (F::R8g8b8a8Premultiplied, true), // Format_RGBX8888
        17 => (F::R8g8b8a8, false),             // Format_RGBA8888
        18 => (F::R8g8b8a8Premultiplied, false), // Format_RGBA8888_Premultiplied
        _ => return None,
    })
}

fn capture(window_id: &str) -> Result<Frame, String> {
    let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|error| error.to_string())?;
    let (mut reader, writer) = std::io::pipe().map_err(|error| error.to_string())?;
    let pipe_fd: OwnedFd = writer.into();
    let fd_list = gio::UnixFDList::from_array([pipe_fd]);

    // The pixels arrive through the pipe while the call is still running, so
    // they must be read at the same time or KWin would block on a full pipe.
    let (done, finished) = channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = done.send(reader.read_to_end(&mut bytes).map(|_| bytes));
    });

    let options: HashMap<String, glib::Variant> = HashMap::from([
        ("include-cursor".to_string(), false.to_variant()),
        ("include-decoration".to_string(), true.to_variant()),
        ("native-resolution".to_string(), false.to_variant()),
    ]);
    let parameters = (window_id.to_string(), options, glib::variant::Handle(0)).to_variant();
    trace(|| format!("calling CaptureWindow for {window_id}"));
    let reply = connection.call_with_unix_fd_list_sync(
        Some("org.kde.KWin"),
        "/org/kde/KWin/ScreenShot2",
        "org.kde.KWin.ScreenShot2",
        "CaptureWindow",
        Some(&parameters),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        Some(&fd_list),
        gio::Cancellable::NONE,
    );
    // Our copy of the write end must be closed for the reader to see the end.
    drop(fd_list);
    let (reply, _) = reply.map_err(|error| error.to_string())?;
    let data = finished
        .recv_timeout(Duration::from_secs(6))
        .map_err(|_| "timed out waiting for the screenshot pixels".to_string())?
        .map_err(|error| error.to_string())?;

    let results = glib::VariantDict::new(Some(&reply.child_value(0)));
    let number = |key: &str| -> Result<usize, String> {
        results
            .lookup::<u32>(key)
            .ok()
            .flatten()
            .map(|value| value as usize)
            .ok_or_else(|| format!("KWin's reply has no {key}"))
    };
    let (width, height, stride) = (number("width")?, number("height")?, number("stride")?);
    let qt_format = number("format")? as u32;
    let (format, padded) = gdk_format(qt_format)
        .ok_or_else(|| format!("unsupported screenshot format {qt_format}"))?;
    if width == 0 || height == 0 || stride < width * 4 || data.len() < stride * height {
        return Err("KWin returned an incomplete screenshot".to_string());
    }

    let (width, height, mut data) = shrink(&data, width, height, stride);
    if padded {
        for pixel in data.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    Ok(Frame {
        window_id: window_id.to_string(),
        width,
        height,
        format,
        data,
    })
}

/// Box-filters a 4-bytes-per-pixel image down to fit `MAX_WIDTH` x `MAX_HEIGHT`.
fn shrink(data: &[u8], width: usize, height: usize, stride: usize) -> (usize, usize, Vec<u8>) {
    let step = width
        .div_ceil(MAX_WIDTH)
        .max(height.div_ceil(MAX_HEIGHT))
        .max(1);
    let (out_width, out_height) = ((width / step).max(1), (height / step).max(1));
    let mut out = vec![0_u8; out_width * out_height * 4];
    for oy in 0..out_height {
        for ox in 0..out_width {
            let mut sum = [0_u32; 4];
            let mut count = 0_u32;
            for y in oy * step..((oy + 1) * step).min(height) {
                let row = y * stride;
                for x in ox * step..((ox + 1) * step).min(width) {
                    let pixel = &data[row + x * 4..row + x * 4 + 4];
                    for channel in 0..4 {
                        sum[channel] += u32::from(pixel[channel]);
                    }
                    count += 1;
                }
            }
            let target = (oy * out_width + ox) * 4;
            for channel in 0..4 {
                out[target + channel] = (sum[channel] / count.max(1)) as u8;
            }
        }
    }
    (out_width, out_height, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrink_keeps_small_images_and_averages_large_ones() {
        let small = vec![10_u8; 4 * 2 * 2];
        let (w, h, out) = shrink(&small, 2, 2, 8);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, small);

        // 720x520 shrinks by 2: each output pixel averages a 2x2 block.
        let mut big = vec![0_u8; 720 * 520 * 4];
        for pixel in big.chunks_mut(8) {
            pixel[..4].copy_from_slice(&[0, 0, 0, 255]);
            pixel[4..].copy_from_slice(&[200, 100, 50, 255]);
        }
        let (w, h, out) = shrink(&big, 720, 520, 720 * 4);
        assert_eq!((w, h), (360, 260));
        assert_eq!(&out[..4], &[100, 50, 25, 255]);
    }

    #[test]
    fn maps_known_qt_formats_only() {
        assert!(gdk_format(6).is_some());
        assert!(gdk_format(3).is_none());
    }
}

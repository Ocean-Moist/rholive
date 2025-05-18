# XCap Crate Comprehensive Guide (v0.6.0)

## Introduction

**XCap** is a cross-platform screen capture library for Rust, supporting Linux (both X11 and Wayland), macOS, and Windows. It provides a unified API to capture screenshots of entire screens (monitors) or individual application windows, and also includes experimental support for screen video recording (work-in-progress). The crate abstracts away the platform-specific details of capturing screen content, allowing developers to write code that works on multiple operating systems without change. This guide serves as both a **user guide** (explaining how to use XCap in your projects) and a **developer guide** (explaining its architecture and internals), and reflects the latest GitHub version (v0.6.0) rather than the older docs.rs documentation.

## Installation

To use XCap in your project, add it to your Cargo dependencies. For example, in your **Cargo.toml**:

```toml
[dependencies]
xcap = "0.6"
```

You can also run `cargo add xcap` to automatically add the crate. By default, XCap comes without extra features enabled. XCap provides an optional feature called **`image`**. Enabling this feature will include the *default features* of the [`image`](https://crates.io/crates/image) crate, which allow you to easily encode/decode images (e.g. saving screenshots as PNG, JPEG, etc.). If you plan to save screenshots to files or manipulate them using the `image` crate's capabilities, consider enabling the `image` feature:

```toml
xcap = { version = "0.6", features = ["image"] }
```

**Platform requirements:** Because XCap uses low-level OS capture APIs, you may need certain system libraries or permissions:

* **Linux (X11):** Ensure that X11 development libraries are available (XCap uses XCB under the hood). This is usually standard on most Linux systems with X11.
* **Linux (Wayland):** Ensure Wayland is available. XCap uses the `libwayshot-xcap` and PipeWire libraries for Wayland capture, so having the Wayland client libraries and PipeWire installed is recommended. (For example, on Debian/Ubuntu you might need packages like `libwayland-client` and `libpipewire` – the XCap CI uses `libwayland-dev`.)
* **macOS:** XCap leverages macOS frameworks (via `objc2` crates) for capture, which are included in macOS by default. You may need to allow screen recording permissions for your application to capture the screen.
* **Windows:** XCap uses the official Windows API (via the `windows` crate). No extra downloads are necessary on Windows, but your program may require graphical desktop context (it won’t work in a pure server context with no screen).

After adding the dependency, you can start using the `xcap` crate in your Rust code by importing the types you need, e.g. `use xcap::Monitor; use xcap::Window;`.

## Basic Usage and Key Features

XCap’s API is designed to be straightforward if you are familiar with Rust. The main types you will use are **`Monitor`** (to work with entire displays/screens) and **`Window`** (to work with individual application windows). Both types provide methods for capturing images (screenshots), and `Monitor` additionally provides methods for capturing sub-regions and starting video recordings. All screen images are returned as `RgbaImage` objects (re-exported from the `image` crate), which you can manipulate or save using the `image` crate’s functionality.

Below are some common usage patterns:

* **Enumerating Monitors:** You can retrieve a list of all available screens/monitors using `Monitor::all()`. For example:

  ```rust
  use xcap::Monitor;
  let monitors = Monitor::all().unwrap();
  for (i, m) in monitors.iter().enumerate() {
      println!("Monitor {}: {} ({}x{})", i, m.name().unwrap(), m.width().unwrap(), m.height().unwrap());
  }
  ```

  This will list all monitors, along with their names and resolutions. The `Monitor::all()` method returns a `Vec<Monitor>` on success. You can also get a specific monitor by a point on the screen with `Monitor::from_point(x, y)`, which returns the monitor that contains the given screen coordinate (useful for determining which monitor the mouse is on, for example).

* **Capturing a Screenshot of a Monitor:** Once you have a `Monitor` object, call `monitor.capture_image()` to capture the entire screen. This returns an `RgbaImage` (an image buffer with RGBA pixels) wrapped in a `Result` for error handling. For example:

  ```rust
  use xcap::Monitor;
  let monitor = Monitor::all().unwrap().into_iter().next().unwrap();  // take the first monitor
  let image = monitor.capture_image().unwrap();
  image.save("screenshot.png").unwrap();
  ```

  In this snippet, we grab the first monitor and save its screenshot to a file. (The `image.save(...)` method is provided by the `image` crate; ensure the `png` feature of `image` is enabled, which is covered by XCap’s `image` feature flag.)

* **Capturing a Region of the Screen:** XCap allows you to capture a sub-region of a screen by specifying an area (x, y, width, height). Use `Monitor::capture_region(x, y, width, height)` to do this. The `(x, y)` coordinates are relative to the top-left corner of that monitor. For example, to capture a 100x100 square region at position (50, 50) on the first monitor:

  ```rust
  let region_img = monitor.capture_region(50, 50, 100, 100).unwrap();
  region_img.save("partial.png").unwrap();
  ```

  This will produce an image of the specified region. **Note:** The coordinates and dimensions must lie within the monitor’s bounds or else an error (`XCapError::InvalidCaptureRegion`) is returned. For instance, requesting a region that extends beyond the screen edges will result in an `InvalidCaptureRegion` error.

* **Enumerating Windows:** You can list all currently open application windows with `Window::all()`. This returns a vector of `Window` objects, sorted by their Z-order (stacking order) from back to front (i.e., the first in the list is the bottom-most window, and the last is the top-most/focused window, depending on platform). For example:

  ```rust
  use xcap::Window;
  let windows = Window::all().unwrap();
  for win in &windows {
      println!("Window ID={} Title='{}' AppName='{}'", 
               win.id().unwrap(), 
               win.title().unwrap_or_default(), 
               win.app_name().unwrap_or_default());
  }
  ```

  Each `Window` has methods to query its properties: `id()` for the native window identifier, `pid()` for the process ID owning the window, `title()` for the window title, `app_name()` for the application name, its position (`x()`, `y()`) and size (`width()`, `height()`), the monitor it’s on (`current_monitor()` returns a `Monitor`), and state flags like `is_minimized()`, `is_maximized()`, and `is_focused()`. Keep in mind that on some platforms (especially Wayland on Linux), enumerating windows and some of these properties might be limited or require special permissions due to security constraints of the window system.

* **Capturing a Screenshot of a Window:** To capture an individual window’s content, use `Window::capture_image()`. This will return an `RgbaImage` of just that window’s client area (what’s visible on screen). For example, if you have obtained a particular `Window` (say the one currently focused):

  ```rust
  if let Some(focused) = windows.into_iter().find(|w| w.is_focused().unwrap()) {
      let win_img = focused.capture_image().unwrap();
      win_img.save("window.png").unwrap();
      println!("Captured window '{}'", focused.title().unwrap());
  }
  ```

  This snippet finds the currently focused window and captures it. Note that if a window is minimized or fully covered by others, the image may be empty or outdated on some platforms. On macOS, for instance, capturing a minimized window might not be possible (the API will likely capture a blank image or fail). On Windows and X11, capturing should work as expected as long as the window is not entirely off-screen.

* **Recording a Video of the Screen:** XCap provides an interface for screen recording via the `VideoRecorder` struct. You obtain a `VideoRecorder` by calling `Monitor::video_recorder()` on the monitor you want to record. This returns a tuple `(VideoRecorder, Receiver<Frame>)`, where `Frame` represents a single video frame (image) and `Receiver<Frame>` is a channel from which you can receive frame data in real-time. For example:

  ```rust
  use std::time::Duration;
  use xcap::{Monitor, VideoRecorder};

  // Start recording the first monitor
  let monitor = Monitor::all().unwrap()[0].clone();
  let (recorder, frame_rx) = monitor.video_recorder().unwrap();
  recorder.start().unwrap();

  // Receive a few frames
  for i in 0..10 {
      if let Ok(frame) = frame_rx.recv_timeout(Duration::from_secs(1)) {
          println!("Received frame {}: {}x{} pixels", i, frame.width, frame.height);
          // You could process the frame.raw bytes here
      } else {
          eprintln!("No frame received (timed out)");
          break;
      }
  }

  recorder.stop().unwrap();
  ```

  In this snippet, we start recording the first monitor, then loop to retrieve 10 frames from the `frame_rx` channel. Each `Frame` contains a raw RGBA pixel buffer (`frame.raw`) along with its width and height. You can process these frames (e.g., encode them into a video file using a codec) as they arrive. Calling `VideoRecorder::stop()` will stop the recording. The `VideoRecorder` runs the capture in a background thread for most platforms, so frames arrive asynchronously via the channel.

  **Important:** Video recording is still marked as **WIP (Work in Progress)** in XCap. This means that while the API exists and works, it may not be fully optimized or 100% stable on all platforms yet. For example, on some systems you might experience lower frame rates or other limitations. As of v0.6.0, XCap supports video capture on all major platforms (Linux X11/Wayland, Windows, macOS), internally using platform-specific APIs (details below). However, be prepared for potential improvements or API changes in future releases as this feature matures.

## Architecture and Internal Design

Under the hood, XCap is carefully structured to abstract away platform differences. Understanding this architecture can be helpful for developers who wish to contribute or troubleshoot at a lower level.

**Platform Abstraction:** The crate defines platform-agnostic structs `Monitor` and `Window` which hold an internal implementation reference. For example, `Monitor` is essentially a wrapper around an internal `ImplMonitor` object, and `Window` wraps an `ImplWindow`. These internal implementations are defined separately for each operating system, under a `platform` module. At compile time, XCap picks the appropriate platform module via conditional compilation. In the source, you’ll find `src/linux/`, `src/macos/`, and `src/windows/` directories, each implementing the required functionality for that OS.

* **Linux:** On Linux, XCap supports both X11 and Wayland display servers. The Linux implementation can detect at runtime whether the environment is Wayland or X11 and will use the appropriate method. For **X11**, XCap uses the XCB library (via the Rust `xcb` crate) to capture screens and window images. It employs techniques like X11’s XImage or XShm (shared memory) to efficiently grab pixels. For **Wayland**, direct capture is not trivial due to security, so XCap integrates with the \[`libwayshot-xcap`] crate and **PipeWire** to capture the screen content. In fact, XCap 0.5.0 introduced support for Wayland screen recording using PipeWire. Additionally, XCap may utilize the desktop portal D-Bus interface (via `zbus`) on Wayland for certain tasks. Note that on some Wayland compositors, window-level capture (enumerating windows and capturing an individual window) may not be available or may require a portal prompt. XCap’s design attempts to handle this gracefully, but results can vary by environment.

* **macOS:** The macOS implementation uses native APIs (via the `objc2` family of crates that wrap Cocoa frameworks). For capturing a screen or window, it likely uses **Core Graphics** (e.g., `CGDisplay` and `CGWindowListCreateImage`) for screenshots. The dependencies also include **AVFoundation/CoreMedia** which suggests video capture may be using Apple’s recording APIs or a timer + capture approach. macOS requires that the application has screen recording permission (the first time you run a capture, macOS will prompt the user). XCap’s `Window::is_focused()` on macOS was improved in v0.4.1 to be real-time, meaning it queries the focused window dynamically rather than caching it.

* **Windows:** The Windows implementation uses the Windows Runtime (through the `windows` crate) to access Win32 and COM interfaces for capturing screen content. For full-screen capture, XCap uses **DXGI Desktop Duplication** APIs (DirectX 11) to efficiently copy the framebuffer of the display. The code shows usage of `IDXGIOutputDuplication::AcquireNextFrame` and related DirectX calls under the hood, which deliver frames for recording. For window capture, Windows provides functions like `BitBlt` (via GDI) or DWM thumbnails; XCap likely uses a compatible approach (there is a utility in the Windows module converting BGRA to RGBA images, indicating it uses BGRA pixel data from Windows which is typical of GDI screenshots). In XCap v0.6.0, the Windows window capture internals were updated to implement `Send` and `Sync` for the internal window handle wrapper, making it safer to use `Window` objects across threads.

* **Coordinate Systems and Units:** XCap normalizes as much as possible. Coordinates (x, y) and dimensions (width, height) are generally in pixel units. On macOS with Retina (HiDPI) displays or on Wayland with scaling, XCap takes scaling into account. For instance, the `Monitor::scale_factor()` method returns the monitor’s pixel scaling factor, and XCap uses this internally to ensure captured images have the expected resolution. The `Monitor::rotation()` method gives the screen rotation in degrees (0, 90, 180, 270) if the display is rotated. These values come from platform APIs (XRandR on X11, etc.).

* **Threading Model:** Capturing a screen is typically a heavy operation (especially for video), so XCap’s implementations use background threads where appropriate. For example, when you obtain a `VideoRecorder` and call `start()`, it spawns a thread that continuously captures frames and sends them through the channel. The `Frame` receiver uses Rust’s standard mpsc channel (`std::sync::mpsc::Receiver`) to deliver frames asynchronously. The `Monitor` and `Window` types themselves are `Clone` and can be passed around; after v0.6.0’s updates, their internal implementations are thread-safe (the Windows implementation required an update to be `Send`/`Sync` as noted). However, keep in mind that actual capture operations should not overlap on the same monitor/window (for example, avoid calling `capture_image` on a monitor from multiple threads concurrently, as underlying OS APIs may not expect that). Use proper synchronization if needed.

* **Error Handling:** XCap defines an error enum `XCapError` to unify various error conditions. This includes generic errors (`XCapError::Error(String)` for custom messages), `InvalidCaptureRegion` for out-of-bounds capture requests, and a variety of wrappers for underlying OS/FFI errors. For instance, on Linux, `XCapError` can wrap X11 errors, Wayland/DBus errors, image conversion errors, etc., via variants like `XCapError::XcbError`, `XCapError::ZbusError`, `XCapError::LibwayshotError`, etc.. On Windows, it wraps the `windows::core::Error` for COM errors, and on macOS it may wrap CoreGraphics errors. This design means most XCap functions return a `XCapResult<T>` alias, which is simply `Result<T, XCapError>`. As a user, you should handle errors (e.g., using `?` or `match`) and be aware that certain functionality might not be available (for example, trying to capture a window on Wayland without the proper permissions will likely yield an error).

## Public API Reference

Below is a detailed reference of XCap’s public interface (v0.6.0), including the key structs, functions, and enums, along with brief descriptions of each. This complements the usage examples above, providing a catalog of capabilities. All functions that interact with the system can potentially fail, returning an `XCapError` wrapped in `XCapResult`. Unless otherwise noted, coordinates and sizes are in pixels.

### Struct `Monitor`

Represents a physical display/screen (or a virtual monitor) attached to the system. Use `Monitor` methods to query properties of the screen and to capture its content.

* **`Monitor::all() -> XCapResult<Vec<Monitor>>`** – Returns a list of all monitors detected on the system. The order is typically as reported by the system (on Windows and macOS this is often an undefined order or primary first; on Linux X11 it’s the order from XRandR; on Wayland all monitors should be listed as well). Each `Monitor` in the list can be used independently.

* **`Monitor::from_point(x: i32, y: i32) -> XCapResult<Monitor>`** – Returns the monitor that contains the point `(x, y)` in global screen coordinates. This is useful for determining which monitor a given pixel or the cursor is on. Coordinates are typically measured from the top-left of the virtual desktop (for multi-monitor setups, (0,0) is usually the top-left of the primary monitor, and other monitors may have positive or negative coordinates depending on arrangement).

* **`Monitor::id(&self) -> XCapResult<u32>`** – Returns an identifier for the monitor. This ID is platform-dependent: on Windows it might be a display index or handle, on X11 it could be the XRandR output ID, etc. Primarily useful for debugging or correlating with OS-specific APIs.

* **`Monitor::name(&self) -> XCapResult<String>`** – Returns the monitor’s name. This might be a human-readable name or model (e.g., "Color LCD" on Mac for the built-in display, or "HDMI-1" / "DP-1" on Linux X11, etc.). Not all platforms provide a friendly name; if unavailable, it may return an empty or generic name.

* **`Monitor::x(&self) -> XCapResult<i32>`**, **`Monitor::y(&self) -> XCapResult<i32>`** – The top-left coordinate of the monitor in the virtual desktop space. For a single monitor setup this is usually (0,0). In multi-monitor setups, monitors can have different (x,y). For example, if you have two monitors side by side, one might have x=0 and the other x equal to the first monitor’s width.

* **`Monitor::width(&self) -> XCapResult<u32>`**, **`Monitor::height(&self) -> XCapResult<u32>`** – The resolution of the monitor in pixels (width and height). This is the current active resolution of the screen.

* **`Monitor::rotation(&self) -> XCapResult<f32>`** – The rotation of the display in degrees. Possible values are typically 0.0, 90.0, 180.0, 270.0. Not all platforms support querying this; on some, a rotated display may still report width/height swapped and rotation 0.

* **`Monitor::scale_factor(&self) -> XCapResult<f32>`** – The pixel scaling factor of the display. On HiDPI displays (like Retina MacBooks or some 4K displays with scaling), this might be 2.0 (meaning coordinates are in points that are 2 actual pixels each). XCap uses this internally, and usually you won’t need to worry about it unless you’re doing something special with DPI.

* **`Monitor::frequency(&self) -> XCapResult<f32>`** – The refresh rate (frequency in Hz) of the monitor. For example, 60.0 for 60Hz. This might be rounded or 0.0 if not available.

* **`Monitor::is_primary(&self) -> XCapResult<bool>`** – Whether this monitor is the primary display (the one with the main menu bar or taskbar, etc., depending on OS).

* **`Monitor::is_builtin(&self) -> XCapResult<bool>`** – Whether this is a built-in display (true for a laptop’s internal screen, false for external monitors; on desktops it’s usually false).

* **`Monitor::capture_image(&self) -> XCapResult<image::RgbaImage>`** – Captures the entire screen (this monitor) and returns it as an `RgbaImage`. This is a single-frame screenshot. The image is in 8-bit RGBA format. Use the `image` crate’s functionality (already in scope via `xcap::image`) to save or process it. This operation may be slow (it has to read potentially millions of pixels from the OS), so avoid calling it in tight loops unless necessary.

* **`Monitor::capture_region(&self, x: u32, y: u32, width: u32, height: u32) -> XCapResult<image::RgbaImage>`** – Captures a sub-region of the monitor. The `(x,y)` is the top-left of the desired region *relative to this monitor’s top-left corner* (not global coordinates), and the region must lie entirely within the monitor bounds. This is useful if you only need a portion of the screen (reducing memory and processing). If the specified region goes outside the screen, this returns an `XCapError::InvalidCaptureRegion` error.

* **`Monitor::video_recorder(&self) -> XCapResult<(VideoRecorder, std::sync::mpsc::Receiver<Frame>)>`** – Initializes a video recorder for this monitor and returns a recorder handle plus a channel **receiver** for `Frame`s. After calling this, you typically call `VideoRecorder::start()` on the returned recorder, then listen on the `Receiver<Frame>` for incoming frames. The `Frame` struct contains `width`, `height`, and `raw` pixel data for each frame. This design was chosen to allow non-blocking capture – frames are produced in the background thread and you can handle them at your own pace. Only one video recorder can be active per monitor at a time. If you need to stop recording, call `VideoRecorder::stop()` on the handle.

### Struct `Window`

Represents an application window on the desktop. Windows can belong to different applications. XCap allows capturing window images and querying window metadata.

* **`Window::all() -> XCapResult<Vec<Window>>`** – Returns a list of all visible (and in some cases, invisible) windows on the system. The windows are sorted by Z-order (their stacking order) from lowest (background) to highest (foreground). On Windows, this means the first item might be the desktop or a background window and the last item is the active window. On X11, the stacking order is determined by the window manager. On macOS, this list typically includes all windows in all spaces (macOS does not easily allow listing minimized windows unless using accessibility APIs – XCap tries to include as much as possible). Note that on Wayland, this may return an empty list or a limited list, as Wayland does not generally allow global window enumeration for security – XCap might only list windows of the current application or use a portal if available.

* **`Window::id(&self) -> XCapResult<u32>`** – Returns an identifier for the window. On Windows, this is the window handle (HWND) cast to a number. On X11, this is the window XID. On macOS, it could be the CGWindowID. These IDs can be used for debugging or cross-referencing with platform-specific calls, but are generally not needed for using XCap itself.

* **`Window::pid(&self) -> XCapResult<u32>`** – Returns the process ID of the application that owns this window. This can be used to identify which application the window belongs to.

* **`Window::app_name(&self) -> XCapResult<String>`** – Returns the name of the application for this window. For example, "chrome" or "firefox" or "Terminal". This is platform-dependent: on Windows it’s the executable name, on macOS the application name, and on Linux it tries to get the application name via window properties or WM\_CLASS. In XCap v0.4.1, a bug with empty titles and incorrect app names (e.g., for Firefox) was fixed, so these values should be more reliable in the latest version.

* **`Window::title(&self) -> XCapResult<String>`** – Returns the window’s title text. This is the text in the title bar of the window (if any). If a window has no title (or it’s an OS window like a desktop background), this might be an empty string.

* **`Window::current_monitor(&self) -> XCapResult<Monitor>`** – Returns the `Monitor` on which the majority of this window is currently displayed. For windows spanning multiple monitors, typically the monitor which contains the center of the window or the largest portion is returned. You can use this to, for example, capture the screen that a window is on or just to identify placement.

* **`Window::x(&self) -> XCapResult<i32>`**, **`Window::y(&self) -> XCapResult<i32>`** – The window’s top-left position on the virtual desktop. This position is in global screen coordinates (like `Monitor::from_point` expects). Note that (x,y) could be negative or beyond a single monitor’s width/height if the window is on a secondary monitor.

* **`Window::z(&self) -> XCapResult<i32>`** – The window’s Z-order (stacking) position. Higher values generally mean the window is above others. The actual units of Z are not particularly meaningful across platforms (it could be an index or a timestamp of focus). Mostly, you would use the ordering of `Window::all()` or check `is_focused` instead of directly using z-values.

* **`Window::width(&self) -> XCapResult<u32>`**, **`Window::height(&self) -> XCapResult<u32>`** – The width and height of the window’s client area in pixels. This does not include any window decorations (title bar, borders) – it’s the drawable area.

* **`Window::is_minimized(&self) -> XCapResult<bool>`** – Returns true if the window is currently minimized/iconified.

* **`Window::is_maximized(&self) -> XCapResult<bool>`** – Returns true if the window is maximized to fullscreen or near-fullscreen size.

* **`Window::is_focused(&self) -> XCapResult<bool>`** – Returns true if this window is the currently focused (active) window. Only one window should return true for this at a time on a desktop (per seat). On macOS, as mentioned, this value is updated dynamically as of v0.4.1 fix. On Linux, some window managers might not expose focus state for all windows; XCap uses best-effort via EWMH hints or compositor APIs.

* **`Window::capture_image(&self) -> XCapResult<image::RgbaImage>`** – Captures an image of the window’s contents. This returns an `RgbaImage` of the same dimensions as `width() x height()`. If the window is partially off-screen or occluded by other windows, the capture will still attempt to get the window’s content (on some platforms like Windows and macOS, it can capture occluded windows via OS APIs; on X11, it captures the pixels currently on screen, which means occluded parts might not be captured unless using compositing). This is a convenient way to screenshot a single window. If the operation fails (for example, on Wayland without permission, or if the window no longer exists), it will return an `Err(XCapError)`.

*Note:* There is no direct `Window::capture_region` method in the public API – if you need a region of a window, you can capture the whole window and then crop the resulting image using the `image` crate. The focus of XCap is to provide full-window or full-screen captures.

### Struct `VideoRecorder` and Struct `Frame`

`VideoRecorder` is a handle used to control screen recording for a monitor. It cannot be constructed directly; you obtain one from `Monitor::video_recorder()` as shown earlier. `Frame` represents a video frame (image buffer).

* **`VideoRecorder::start(&self) -> XCapResult<()>`** – Starts the recording process. Internally, this signals the background capture thread (which was created when the recorder was initialized) to begin grabbing frames. The frames will be sent through the `Receiver<Frame>` channel that was paired with this recorder. If recording is already started, calling `start()` might have no effect or return an error (depending on platform implementation). Generally, call this once after you get the recorder.

* **`VideoRecorder::stop(&self) -> XCapResult<()>`** – Stops the recording loop. This signals the capture thread to pause or terminate (frames will stop arriving on the channel). After calling `stop()`, you can drop the `VideoRecorder`. If you need to resume recording, you would currently have to create a new recorder via `Monitor::video_recorder()` again (the API does not provide a `pause/resume`, just start/stop).

* **`Frame` (fields)** – A `Frame` object has the following public fields: `width: u32`, `height: u32`, and `raw: Vec<u8>`. The `raw` vector contains the pixel data in RGBA8 format (same format as `RgbaImage` uses: 4 bytes per pixel in R,G,B,A order). Typically, you might convert this `raw` into an `image::RgbaImage` by doing `let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.raw).unwrap()`, or feed it to a video encoder. Each frame usually represents one monitor frame update (however, the capture rate might be lower than the monitor’s refresh rate due to performance limits).

**Performance Consideration:** Capturing and recording can generate a lot of data (e.g., a 1920x1080 frame is about 8 MB in RGBA). XCap does not compress or encode frames – it provides raw frames so you can choose what to do (save to images, feed to a video encoder, etc.). Make sure to handle frames promptly or buffer them carefully to avoid memory bloat if the receiver side is slower than the capture. You can always drop frames by simply reading and ignoring if you can't process them fast enough.

### Enum `XCapError` and Type Alias `XCapResult`

`XCapError` is an enumeration of error kinds that XCap functions might return. It implements the standard `Error` trait (via `thiserror`), so it can be used with the `?` operator in Result chains. Notable variants include:

* **`XCapError::Error(String)`** – A general error with a message. Many high-level checks will return this for unexpected situations (the string usually provides details).

* **`XCapError::InvalidCaptureRegion(String)`** – Returned when a requested capture region is invalid (for example, coordinates out of bounds, or width/height is 0). The string typically includes specifics, like the invalid values or bounds.

* **`XCapError::StdSyncPoisonError(String)`** – Used internally when a mutex is poisoned (should be rare for user code to see, but it’s there due to the use of threads and `Mutex` in the implementation).

* **Platform-specific error variants:** XCapError has a number of variants that wrap lower-level library errors. These are enabled per target OS. For instance:

    * On Linux: `XCapError::XcbError(...)` and `XCapError::XcbConnError(...)` wrap errors from the XCB library; `XCapError::ZbusError(...)` for DBus/Portal errors; `XCapError::LibwayshotError(...)` for Wayland capture library errors; `XCapError::PipewireError(...)` for PipeWire-related errors, etc. Many of these simply encapsulate the original error (using the `#[from]` attribute for automatic conversion).
    * On macOS: `XCapError::Objc2CoreGraphicsCGError(CGError)` wraps a CoreGraphics error code if a CG capture call failed.
    * On Windows: `XCapError::WindowsCoreError(windows::core::Error)` for COM/Win32 errors, and `XCapError::Utf16Error` for string conversion issues.

All functions in XCap return results as `XCapResult<T>` which is a type alias for `Result<T, XCapError>`. When writing code, you can handle these with idiomatic Rust patterns:

```rust
match monitor.capture_image() {
    Ok(img) => { /* use the image */ },
    Err(XCapError::InvalidCaptureRegion(msg)) => { println!("Invalid region: {}", msg); },
    Err(e) => { eprintln!("Capture failed: {}", e); }
}
```

Or simply using `?` to propagate the error to your caller if you choose not to handle it at that point.

## Notable Changes in Latest Version (vs 0.4.1)

If you have used XCap in the past (particularly version 0.4.1 or earlier), here are the major enhancements and changes in the latest release (v0.6.0):

* **Wayland Support for Recording:** Early versions of XCap (<=0.4.1) introduced screen recording for Linux but it was limited. In v0.5.0, XCap added support for **Wayland screen recording** via PipeWire. This means if your application runs under Wayland (e.g., GNOME, KDE in Wayland mode), XCap can now record the screen (whereas before it might have only worked under X11). Ensure the appropriate PipeWire permissions (on some systems you may need to use a portal).

* **Region Capture API:** In v0.6.0, XCap exposed a lower-level capability to capture custom regions of the screen. The high-level manifestation of this is the `Monitor::capture_region` method, which was not available in 0.4.1. This allows you to get a subsection of the monitor’s image directly, which can be more efficient than capturing the whole screen and cropping later.

* **Public `Frame` struct:** As of v0.5.2, the `Frame` struct (used for video frames) was made part of the public API. In 0.4.x, the video recording was even more experimental and the frame type was not exposed to users. Now you can directly use `Frame` (accessible via the channel from `video_recorder`) to inspect width, height, and pixel data of each frame, giving you more flexibility in how to handle recorded frames.

* **Thread Safety Improvements:** The internal implementations, especially on Windows, have been refined. For example, v0.6.0 implements `Send` and `Sync` for the internal window capture implementation on Windows, which resolves potential multithreading issues (ensuring you can move `Window` objects across threads safely). Generally, the library has become more robust in multi-threaded contexts since 0.4.1.

* **Bug Fixes and Accuracy:** Several bugs present in 0.4.1 were fixed in subsequent releases. Notably, window title and app name detection on Linux has improved (fix for empty titles and correct app name for Firefox windows). The `Window::is_focused()` on macOS now reflects real-time focus changes rather than being static. Support for capturing on certain Wayland compositors (like wlroots-based compositors) was added, whereas older versions might not work on those systems.

* **Documentation and Examples:** The latest version has more comprehensive documentation (like this guide) and includes example programs in the repository for common tasks (listing monitors/windows, capturing, recording, etc.). This reflects a maturation of the project from 0.4.1, which had minimal documentation.

* **Internal Updates:** XCap’s Rust edition was updated (to Rust 2021 edition in a maintenance commit), and dependencies like the `windows` crate and `objc2` were kept up-to-date, which may reduce issues and improve performance on their respective platforms. These internal changes shouldn’t affect the public API usage, but they ensure XCap stays compatible with modern Rust and OS changes.

In summary, if you used XCap 0.4.1, upgrading to 0.6.0 brings more features (especially for Wayland and region capture) and more stability. The core usage of the API (Monitors, Windows, capture\_image, etc.) remains the same, so your 0.4.x code should mostly continue to work, but you can now take advantage of the new capabilities outlined above.

## Conclusion

XCap provides a powerful, unified way to capture screens and windows across all major desktop platforms using Rust. Whether you need to take screenshots of multiple monitors or build a cross-platform screen recording tool, XCap’s API is designed to be ergonomic and consistent. As of v0.6.0, the crate is quite feature-rich for image capture and has a solid foundation for video capture. While the video recording features are still marked as **experimental**, they open the door for creative applications (like building your own streaming or screen-casting utility in Rust) using the same library on every OS.

We’ve covered how to install XCap, retrieve monitors and windows, capture images, record video frames, and we delved into the architecture that makes it all work. For further details, you can refer to the [official repository](https://github.com/nashaofu/xcap) which may contain additional documentation, and the release notes for the latest changes. Being an open-source project, contributions and issue reports are welcome – the library is under active development, as evidenced by the steady stream of improvements and fixes.

With this guide, you should be well-equipped to integrate XCap into your project and utilize its capabilities fully. Happy capturing with XCap!

**Sources:** The information in this document was compiled from the XCap GitHub repository and release notes, including the source code for version 0.6.0 and the official changelog. The descriptions of functionality are based on the documented API and code behavior in the latest version. For any deeper technical questions, consulting the source code and issues in the repo can provide further insight.

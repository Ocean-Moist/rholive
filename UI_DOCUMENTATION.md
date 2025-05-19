we use egui_window_glfw_passthrough + glow

Understood. I’ll write a reference-style offline documentation that mimics Rust API docs for `egui_window_glfw_passthrough` used with `glow`, tailored to your use case of creating a transparent, always-on-top overlay window with mouse passthrough and UI elements. I’ll include module-level summaries, struct and function explanations, and inline examples. I’ll let you know when it’s ready.


# `egui_window_glfw_passthrough` + `glow` Integration Guide

**Overview:** This guide demonstrates how to use the `egui_window_glfw_passthrough` crate together with `glow` (OpenGL) to create a transparent, always-on-top overlay window on Linux (X11) with mouse input passthrough. We will cover the architecture of the involved crates, the key types (`GlfwBackend`, `GlfwConfig`, etc.), initialization steps, event processing, high-DPI handling, toggling mouse passthrough, and provide a minimal complete example. The goal is to render an egui UI (text, buttons, etc.) in a borderless overlay window that can be toggled to allow clicks to "pass through" to underlying windows when the overlay is not actively in use.

## Architecture and Components

This setup involves multiple crates working together:

* **`egui_window_glfw_passthrough`** – Window backend for egui using a modified GLFW. It creates and manages an **OpenGL** window via `glfw` with special settings (transparent framebuffer, always-on-top, no decorations, etc.) and enables **mouse passthrough** functionality. It re-exports the `glfw` crate (as `egui_window_glfw_passthrough::glfw`) which includes GLFW window management and event types.
* **`glfw-passthrough` (dependency)** – A fork of the GLFW bindings that adds support for transparency and mouse click-through. This is used under the hood by `egui_window_glfw_passthrough` to achieve overlay capabilities.
* **`egui`** – The immediate mode GUI library for rendering UI elements. You will use `egui::Context` to build your UI each frame (e.g. windows, panels, labels, buttons).
* **`glow`** – A cross-platform OpenGL function loader. This is used to interface with the OpenGL context provided by GLFW. We create a `glow::Context` to issue GL calls for rendering.
* **`egui_glow`** – An egui rendering backend that uses glow/OpenGL. In particular, we use `egui_glow::Painter` to paint egui’s shapes and text on our OpenGL window.

**How they work together:** `egui_window_glfw_passthrough` sets up the GLFW window and converts input events to egui. The `glow` context allows us to use OpenGL in Rust, and the `egui_glow::Painter` knows how to draw egui’s output (textures, shapes) into the OpenGL window. The overlay window is configured to be transparent and always on top, so it floats above other windows. When mouse passthrough is enabled, the overlay window ignores mouse events, letting them reach whatever window is behind the overlay, effectively making the overlay "click-through". When passthrough is disabled, the overlay receives input normally (allowing interaction with the egui UI).

## Key Types and Structures

**`GlfwBackend`** – The primary struct provided by `egui_window_glfw_passthrough` for window management. It encapsulates a GLFW window and the event loop integration for egui. Creating a `GlfwBackend` initializes GLFW (if not already), creates a window with the requested settings, and prepares it for use with egui. The `GlfwBackend` holds:

* A `glfw::Window` (the actual window handle) with an OpenGL context.
* An event receiver for GLFW events (so you can poll input events).
* Possibly internal state for egui integration (e.g. tracking modifier keys).

You will use `GlfwBackend` to create the window and then access the inner `glfw::Window` for further operations (like swapping buffers, toggling settings, etc.). In a simple usage, you can treat `GlfwBackend` as the owner of the window and event loop.

**`GlfwConfig`** – Configuration structure for creating a `GlfwBackend` (and its window). This struct allows you to specify most window properties at startup. Common fields include:

* `width` and `height` (initial window size in pixels or logical units).
* `title` (window title string, for identification).
* `decorated` (bool, whether to show window border/title bar; for an overlay HUD you typically set this to `false` to hide borders).
* `always_on_top` (bool, whether the window should be always on top of other windows – this uses GLFW's “floating” attribute; set `true` for overlays).
* `transparent` (bool, whether the window’s framebuffer should support transparency. Set this to `true` for an overlay so that the background can be alpha-blended with what's behind the window).
* `resizable` (bool, if the window can be resized by the user; overlays often set this to `false`).
* Other options: there may be fields for vsync or OpenGL context version/profile if needed. By default, the config will choose a default OpenGL context (usually GLFW defaults to OpenGL 3.3 core on desktop) and enable vsync for you.

`GlfwConfig` provides sensible defaults via `Default::default()`. You can adjust the fields after creation. For example:

```rust
use egui_window_glfw_passthrough::GlfwConfig;
let mut config = GlfwConfig::default();
config.width = 1280;
config.height = 720;
config.title = "Overlay HUD".into();
config.transparent = true;
config.decorated = false;
config.always_on_top = true;
```

This config sets up a 1280x720 borderless transparent window that will stay on top of other windows.

**GLFW Re-exports:** The crate re-exports the entire `glfw` module. You will use types like `glfw::Window`, `glfw::WindowEvent`, `glfw::Key`, etc., mostly when handling events and controlling the window. For example, you might call `glfw::Window::set_mouse_button_polling` or `window.swap_buffers()` on the re-exported GLFW window.

**OpenGL Context (glow):** After creating the window, you will obtain a `glow::Context` tied to the GLFW window’s OpenGL context. This is not a type from `egui_window_glfw_passthrough` but a crucial piece. The `glow::Context` is created by loading OpenGL function pointers via GLFW (more on this in the initialization steps).

**`egui_glow::Painter`** – This struct is responsible for translating egui’s output (mesh data and textures) into actual OpenGL draw calls. We will create a Painter after setting up the glow context. It provides methods like `paint_and_update_textures` that we will call every frame to render the egui UI. (It internally handles things like shader compilation, setting up buffers, textures for egui fonts, etc.)

With these components, the plan is:

1. Use `GlfwBackend` (with `GlfwConfig`) to create an OpenGL window ready for an overlay.
2. Set up a `glow::Context` from that window’s OpenGL context.
3. Create an `egui_glow::Painter` to handle rendering.
4. Create an `egui::Context` for generating UI.
5. Loop: poll events via `GlfwBackend`/GLFW, feed them to egui, draw egui with the Painter, and toggle passthrough as needed.

## Initialization Steps

Let's walk through the initialization step by step with code snippets:

1. **Configure and create the window backend:** Start by constructing a `GlfwConfig` (as shown above) with the desired settings. Then create the `GlfwBackend`:

   ```rust
   use egui_window_glfw_passthrough::{GlfwBackend, GlfwConfig};
   // ... (configure as above)
   let config = GlfwConfig {
       width: 1280,
       height: 720,
       title: "Overlay HUD".into(),
       transparent: true,
       decorated: false,
       always_on_top: true,
       // other fields default...
       ..Default::default()
   };
   // Create the GLFW backend and window.
   let mut glfw_backend = GlfwBackend::new(config);
   ```

   This will initialize GLFW (the modified `glfw-passthrough`), apply the window hints (transparent framebuffer, floating window, no decor), and open the window. It will also create an OpenGL context for the window. After this, `glfw_backend` holds our window and event receiver. If creation fails (for example, if OpenGL context creation fails), it would typically return an `Err` – in practice you can unwrap or handle the Result.

   *Note:* In some versions, `GlfwBackend::new` might take two parameters (the window config and a graphics config). If so, you can usually pass `GlfwConfig` and a default graphics config (e.g. something like `BackendConfig::default()`). For an OpenGL+glow setup, the default graphics config suffices since it will use the standard OpenGL context. Check the exact function signature in the crate version you are using. In the latest versions, it often requires just the `GlfwConfig`.

2. **Access the window handle:** You can get the underlying `glfw::Window` from the backend. For example:

   ```rust
   let window = glfw_backend.window();  // Assuming a getter method or public field
   // or if GlfwBackend exposes it as pub, then:
   // let window = &mut glfw_backend.window;
   ```

   We need this `window` to set up OpenGL and control buffer swapping. The `GlfwBackend` typically also provides access to the `glfw` context and the event receiver:

   ```rust
   let glfw = glfw_backend.glfw();      // the glfw::Glfw context
   let events = glfw_backend.events();  // Receiver for window events
   ```

   (If direct getters are not available, the fields may be public: e.g. `glfw_backend.window`, `glfw_backend.glfw`, `glfw_backend.events`.)

3. **Make the GL context current:** If not already done by `GlfwBackend::new`, ensure the window’s OpenGL context is current on the thread before using `glow`. Usually, after creating the window, you call:

   ```rust
   window.make_current();
   ```

   This is necessary so that OpenGL function loading will target this context, and so that rendering calls affect the correct window. In many cases, the `GlfwBackend` may have already called `make_current()` internally for convenience. It doesn’t hurt to call it again to be sure.

4. **Create the glow context:** Use GLFW to load OpenGL symbols into a `glow::Context`. The `glfw::Window` provides a method to get function addresses for OpenGL (`get_proc_address`). We pass that to `glow`:

   ```rust
   use egui_window_glfw_passthrough::glfw::Context; // for `get_proc_address`
   use glow::Context as GlowContext;
   // Safety: creating a glow context requires a current OpenGL context on the thread
   let glow_ctx = unsafe {
       GlowContext::from_loader_function(|s| window.get_proc_address(s) as *const _)
   };
   ```

   This creates a `glow::Context` by loading all the necessary OpenGL function pointers via GLFW. We do an `unsafe` block because OpenGL calls are inherently unsafe and we have to guarantee the context is current (which we did with `make_current`).

5. **Instantiate the egui painter:** Now that we have `glow_ctx`, we can create the `egui_glow::Painter`. This will compile the required shaders and set up buffers needed for egui rendering. The Painter expects an `Arc<glow::Context>` or similar, so we usually wrap our `glow_ctx`:

   ```rust
   use egui_glow::Painter;
   use std::sync::Arc;
   let glow_ctx_arc = Arc::new(glow_ctx);
   let mut painter = Painter::new(glow_ctx_arc, "") 
       .expect("Failed to create egui_glow Painter");
   ```

   In newer versions, `Painter::new` may take additional arguments such as a shader prefix or custom shader version. The above call uses an empty shader prefix and default shader version. Adjust if necessary (for example, on some platforms you might pass `"#version 140"` or similar as the shader version string if defaults fail).

   The important part is that we now have a `painter` object which we will use each frame to paint egui UI. It’s a good idea to also call `painter.clear()` once here to clear the window to fully transparent (in case any default content is present) – though after we render the first frame, it will anyway be drawn over.

6. **Create the egui context:** Finally, set up an `egui::Context` which holds the UI state (this is sometimes called `ctx`). This is where all UI configuration and state will live (e.g. memory of widgets, input state, etc.). Simply do:

   ```rust
   let mut egui_ctx = egui::Context::default();
   ```

   We will use `egui_ctx` to start new frames and build UI each loop iteration.

At this point, initialization is complete: we have a window (transparent, always-on-top), an OpenGL context with glow, an egui painter for OpenGL, and an egui context for UI. Next, we proceed to the event loop, where we handle input and rendering.

## Event Handling and Egui Input Processing

Processing input events from GLFW and feeding them to egui is a crucial part of integration. GLFW will produce events for key presses, mouse movement, clicks, etc., which we must convert into `egui::Event` and other input data for egui to consume each frame.

**Polling events:** The `GlfwBackend` provides an event receiver (usually an `mpsc::Receiver<(f64, glfw::WindowEvent)>`) where window events are sent. Each loop iteration, we need to poll GLFW for new events and collect them. For example:

```rust
glfw_backend.glfw().poll_events();
for (_, event) in glfw::flush_messages(&glfw_backend.events) {
    // handle each `glfw::WindowEvent`
}
```

The `glfw::flush_messages` utility drains all pending events from the receiver. Each event comes with a timestamp (we ignore it here, `_`). Now we match on `event` (of type `glfw::WindowEvent`) and translate it to egui:

* **Window close or escape:** If the event indicates the window should close (e.g. user pressed the close button or hit Escape), you may want to break out of your loop. In our overlay case with no decoration, the close event might not happen unless we programmatically trigger it. But checking `window.should_close()` each loop is wise to know when to quit.

* **Resizing events:** GLFW can emit `FramebufferSize` or `ContentScale` events when the window is resized or moved to a monitor with different DPI. We will handle resizing in the rendering section by querying the size each frame, so explicit handling here is optional. However, you might want to note if a resize occurred to update projection matrices or simply redraw.

* **Keyboard input:** For `WindowEvent::Key(key, scancode, action, modifiers)`, you can convert it to an egui key event:

    * Use `egui_window_glfw_passthrough::glfw_to_egui_key(key)` to map a `glfw::Key` to an `egui::Key` (if one exists; not all keys have an egui equivalent).
    * Determine if the key was pressed or released: `egui_window_glfw_passthrough::glfw_to_egui_action(action)` returns `Some(true)` for press, `Some(false)` for release, and `None` for repeat events (you can treat repeat as “press” or ignore).
    * Get modifier keys: `egui_window_glfw_passthrough::glfw_to_egui_modifiers(modifiers)` will convert GLFW modifier flags (Ctrl/Shift/Alt) to an `egui::Modifiers` struct.
    * If the key maps to an egui key, push an `egui::Event::Key` event with the above info into egui’s input.

  For example:

  ```rust
  if let glfw::WindowEvent::Key(glfw_key, _scancode, action, mods) = event {
      if let Some(egui_key) = egui_window_glfw_passthrough::glfw_to_egui_key(glfw_key) {
          if let Some(pressed) = egui_window_glfw_passthrough::glfw_to_egui_action(action) {
              let egui_mods = egui_window_glfw_passthrough::glfw_to_egui_modifiers(mods);
              egui_input.events.push(egui::Event::Key {
                  key: egui_key,
                  pressed,
                  modifiers: egui_mods,
              });
          }
      }
  }
  ```

  (Here `egui_input` is an `egui::RawInput` we are constructing for this frame, described below.)

* **Character input:** For text input (e.g. typing letters), GLFW typically sends `WindowEvent::Char(character)` events. You should feed these to egui as `egui::Event::Text(characters)`. For instance:

  ```rust
  if let glfw::WindowEvent::Char(character) = event {
      egui_input.events.push(egui::Event::Text(character.to_string()));
  }
  ```

  This ensures that typing letters or symbols will insert text into egui text boxes.

* **Mouse buttons:** For `WindowEvent::MouseButton(button, action, modifiers)`, convert the button via `glfw_to_egui_pointer_button(button)` which yields an `Option<egui::PointerButton>`. If the button is one that egui handles (left, right, middle), create an `egui::Event::PointerButton`:

  ```rust
  if let glfw::WindowEvent::MouseButton(btn, action, mods) = event {
      if let Some(egui_btn) = egui_window_glfw_passthrough::glfw_to_egui_pointer_button(btn) {
          let pressed = action == glfw::Action::Press;
          let egui_mods = egui_window_glfw_passthrough::glfw_to_egui_modifiers(mods);
          let pos = last_cursor_pos; // you need to track the latest cursor position
          egui_input.events.push(egui::Event::PointerButton {
              pos,
              button: egui_btn,
              pressed,
              modifiers: egui_mods,
          });
      }
  }
  ```

  *Note:* We need the cursor position (`pos`) for the click event. GLFW’s mouse button event doesn’t directly include the cursor coordinates at that moment, so you should keep a variable that updates on `CursorPos` events (described next) to know the last known cursor position.

* **Cursor movement:** For `WindowEvent::CursorPos(x, y)` events, update a stored cursor position and also push an `egui::Event::PointerMoved`:

  ```rust
  if let glfw::WindowEvent::CursorPos(x, y) = event {
      // Update tracked position
      last_cursor_pos = egui::pos2(x as f32, y as f32);
      egui_input.events.push(egui::Event::PointerMoved(last_cursor_pos));
  }
  ```

  This informs egui where the mouse cursor is. If the cursor goes outside the window, GLFW might send a `CursorEnter(false)` event or no events; you can handle `CursorEnter` to push `egui::Event::PointerGone` when the cursor leaves the window area:

  ```rust
  if let glfw::WindowEvent::CursorEnter(false) = event {
      egui_input.events.push(egui::Event::PointerGone);
  }
  ```

* **Scroll (mouse wheel):** For `WindowEvent::Scroll(x_offset, y_offset)`, send egui a scroll event. Egui expects scrolling in points. Typically, the y\_offset corresponds to vertical scroll (y is often how many lines to scroll):

  ```rust
  if let glfw::WindowEvent::Scroll(_x, y) = event {
      egui_input.events.push(egui::Event::Scroll(egui::vec2(0.0, y as f32 * 20.0)));
      // Multiply by 20.0 to convert scroll steps to pixels; adjust as needed
  }
  ```

  The conversion factor (here 20.0) can be tuned; it represents how many logical pixels one scroll tick should scroll. This may depend on your application or user settings.

Using the helper functions from the crate (`glfw_to_egui_key`, etc.) simplifies mapping. If those are not available, you would manually map GLFW’s key codes to egui’s and handle the logic similarly.

By collecting all these events and pushing them into an `egui::RawInput` (via its `events` vector), you prepare egui to handle the user input for that frame.

## Frame Rendering and High-DPI (HiDPI) Considerations

After processing events, the next step each loop iteration is to begin a new egui frame, build the UI, and then render it.

**Setting up `RawInput` each frame:** Before calling `egui_ctx.begin_frame(...)`, we need to set up an `egui::RawInput` with the latest input state. In addition to the events we collected, RawInput should include:

* `screen_rect`: the current screen area for egui in points (logical pixels). This is typically the full window. We can get the window logical size via `window.get_size()`, which returns `(width, height)` in screen coordinates (points). Use this to set `raw_input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32)))`.
* `pixels_per_point`: the device pixel ratio (DPI scale). This is crucial for HiDPI displays. Compute it each frame as the ratio of framebuffer size to window size. GLFW provides `window.get_framebuffer_size()` (in physical pixels) and `window.get_size()` (logical size). For example, if on a Retina display the logical size is 1280x720 but the framebuffer is actually 2560x1440, then `pixels_per_point = 2560/1280 = 2.0`. Set `raw_input.pixels_per_point = Some(scale_factor)`. On standard DPI (1:1), this will be 1.0. Egui will use this to scale fonts and UI appropriately.
* `time`: you can optionally supply the time (in seconds) for the frame (for animations). Not critical for basic use; egui can function without it.
* `mods` (modifier keys state): egui might keep track of e.g. if Ctrl is held. You can update `raw_input.modifiers` each frame from e.g. the last event or use egui’s internal tracker. If using the helper functions and pushing events for key presses, egui will handle mod state for you.

Example of preparing RawInput:

```rust
let (win_width, win_height) = window.get_size();
let (fb_width, fb_height) = window.get_framebuffer_size();
let pixels_per_point = fb_width as f32 / win_width as f32;
let mut raw_input = egui::RawInput {
    screen_rect: Some(egui::Rect::from_min_size(
        egui::Pos2::new(0.0, 0.0),
        egui::vec2(win_width as f32, win_height as f32),
    )),
    pixels_per_point: Some(pixels_per_point),
    // populate the rest with default/empty:
    ..Default::default()
};
// Attach the events we collected to raw_input
raw_input.events = egui_events;
```

Here `egui_events` is a `Vec<egui::Event>` we built from the GLFW events as described. We then pass `raw_input` into `egui_ctx.begin_frame(raw_input)` to start the UI frame:

```rust
egui_ctx.begin_frame(raw_input);
```

**Building the UI:** Now, between `begin_frame` and `end_frame`, we use egui to construct our UI for this frame. This could involve creating windows, panels, and widgets. For an overlay, you might use a fixed-position window or panel. For example:

```rust
egui::Window::new("Overlay")
    .fixed_pos(egui::pos2(50.0, 50.0))  // position on screen
    .collapsible(false)
    .resizable(false)
    .title_bar(false)  // maybe hide title bar for a cleaner HUD
    .show(&egui_ctx, |ui| {
        ui.label("Overlay HUD is running.");
        if ui.button("Click me").clicked() {
            println!("Button was clicked!");
        }
    });
```

This is just an example: it creates a window at (50,50) with a label and a button. In a real HUD, you might update text or show dynamic info. You can also set the window background to be semi-transparent by adjusting the `ui.style()` or using a `Frame` with a transparent fill. By default, a window will have a slightly transparent background if `config.transparent = true` was set, because the window’s clear color is fully transparent and egui’s default visuals typically use a translucent window fill. You can customize colors via `egui_ctx.set_visuals()` if needed.

After constructing the UI, finalize the frame:

```rust
let full_output = egui_ctx.end_frame();
let paint_jobs = full_output.shapes;        // all the shapes to draw (egui::ClippedShape)
let textures_delta = full_output.textures_delta;  // any font or image texture updates
```

Depending on the egui version, the exact return types might differ slightly:

* In egui 0.30+, `end_frame()` returns a `FullOutput` which contains `platform_output` (for e.g. clipboard operations or window close requests) and `textures_delta` and `shapes`.
* We call `egui_ctx.tessellate(shapes)` to convert those shapes into GPU-friendly primitives (a list of `egui::ClippedPrimitive`). Some versions skip this step by providing clipped primitives directly. For simplicity:

  ```rust
  let clipped_primitives = egui_ctx.tessellate(paint_jobs);
  ```

Now we have `clipped_primitives` (geometry to draw) and `textures_delta` (any changes to egui-managed textures, such as new fonts or image updates). We will feed these to the `Painter`.

**Rendering with glow Painter:** Before painting, ensure the OpenGL viewport matches the framebuffer size (especially after resizes). The egui\_glow painter’s `paint_and_update_textures` will handle viewport and scissor if given the correct screen size in pixels and scale. We call:

```rust
painter.paint_and_update_textures(
    [fb_width as u32, fb_height as u32],  // physical screen size in px
    pixels_per_point,
    &clipped_primitives,
    &textures_delta,
).expect("Painter failed to paint");
```

This call will:

* Upload any pending texture changes (from `textures_delta`) to the GPU (e.g. new glyph atlases).
* Render all the `clipped_primitives` (meshes of triangles) via OpenGL to the screen.
* Use the provided `pixels_per_point` to adjust for DPI in case needed internally.
* Clear the screen or relevant areas if necessary (the painter usually does not fully clear the color buffer unless needed – since we want transparency, it might rely on the fact that unused areas are simply not drawn, leaving the last frame’s pixels. To ensure a fully transparent background each frame, you might want to clear manually).

After painting, you should **swap the buffers** to display the rendered frame:

```rust
window.swap_buffers();
```

This presents the drawn content on the overlay window. If you enabled vsync (by default, `GlfwBackend` might call `glfwSwapInterval(1)`), this will sync to the monitor’s refresh rate (commonly 60Hz), reducing CPU usage.

**Handling HiDPI Resizing:** If the window is moved to a different DPI monitor or resized, `pixels_per_point` can change. Our loop already recomputes it each frame using the latest sizes, so egui will adapt UI scaling on the fly. The `egui_glow::Painter` will also recreate its internal framebuffer for proper resolution if needed (in some cases, the painter uses an intermediate framebuffer for anti-aliasing or for reading pixels; the method `painter.intermediate_fbo()` can be used to set a custom target, but by default it handles window drawing). In summary, simply updating the values each frame as shown will handle DPI changes.

**Note on transparency:** Because we set the GLFW window with a transparent framebuffer, any pixel not drawn by our UI will remain transparent, showing whatever is behind the window. Egui’s default style for windows/panels includes a non-zero alpha (e.g. 85% opaque dark background). If you want parts of the UI to be see-through, adjust the `Visuals` (for example, set `egui_ctx.set_visuals(egui::Visuals::dark())` and then modify `visuals.widgets.noninteractive.bg_fill` alpha). The clear color of the window should be fully transparent at the start of each frame. You can call `gl::ClearColor(0.0, 0.0, 0.0, 0.0)` and `gl::Clear(GL_COLOR_BUFFER_BIT)` (or use glow for the same) at the top of each frame to ensure the background is cleared to transparent. Alternatively, configure the egui painter or context to clear with transparent. The `egui_glow::Painter` might not automatically clear the color buffer (it typically only draws the provided shapes), so doing an explicit clear at the start of the frame is often wise for a transparent overlay:

```rust
unsafe {
    glow_ctx.clear_color(0.0, 0.0, 0.0, 0.0);
    glow_ctx.clear(glow::COLOR_BUFFER_BIT);
}
```

(Ensure to do this *after* making the GL context current and before calling `painter.paint_and_update_textures`.)

## Mouse Passthrough Toggling (`set_mouse_passthrough`)

One of the key features of this crate is the ability to toggle “mouse passthrough” on the window. When enabled, the overlay window will ignore mouse interactions – clicks and hover events will pass through to whatever is behind the overlay. This is extremely useful for HUD overlays: you can display information without blocking the user’s interaction with the underlying application unless needed.

The `glfw-passthrough` integration provides a method to control this: **`window.set_mouse_passthrough(bool)`**. You can call this on the `glfw::Window` to enable or disable the passthrough behavior at runtime. For example:

```rust
// Enable mouse click-through:
window.set_mouse_passthrough(true);
// ... Disable it (overlay will capture mouse again):
window.set_mouse_passthrough(false);
```

When the window is created via `GlfwBackend`, if `GlfwConfig.transparent` was true, the window is likely created with `GLFW_MOUSE_PASSTHROUGH` initially *disabled* (so that you can interact with the UI). You can choose to enable it immediately if you want the overlay to start in a non-interactive mode.

A common pattern is to toggle passthrough automatically based on whether egui needs input. For instance, you might enable passthrough whenever the UI has no interactive hover or focus, and disable it when the user moves the mouse over the UI or is interacting with it. Egui provides signals for this:

* `egui_ctx.wants_pointer_input()` returns `true` if the UI is currently interested in pointer events (e.g. the cursor is hovering a button, or a drag is in progress).
* `egui_ctx.wants_keyboard_input()` similarly for keyboard focus (e.g. a text field is active).

By checking these, you can dynamically set passthrough each frame. For example, at the end of the frame (after calling `end_frame`):

```rust
let wants_input = egui_ctx.wants_pointer_input() || egui_ctx.wants_keyboard_input();
window.set_mouse_passthrough(!wants_input);
```

In this snippet, if egui is not expecting any input (no widget is hovered or active), `wants_input` will be false and we set `set_mouse_passthrough(true)` (allowing clicks to go through). If the user moves the cursor over the UI or is interacting (thus egui wants input), we disable passthrough (`false`), so the overlay will catch the input. This creates an automatic behavior: the overlay is interactive only when needed, otherwise completely transparent to input.

**Example use case:** Imagine an overlay with some controls that the user rarely adjusts. You want the user to be able to click "through" it most of the time (to interact with the game or application behind it), but if they move their mouse over a control or press a hotkey to interact with the overlay, it should accept input. The above logic achieves that seamlessly.

Alternatively, you can implement a manual toggle (e.g. pressing a certain key to enable/disable input passthrough mode). In that case, just call `set_mouse_passthrough` based on your toggle condition.

One more note: Keyboard events also are blocked when passthrough is enabled because the window will not receive focus for input. On Linux/X11, a fully passthrough window won’t become focused on click (since clicks go through), so keyboard input will typically go to the background window as well. Therefore, use `wants_keyboard_input` similarly to decide if you need to grab keyboard (you might also need to focus the window, which can be done by `window.focus()` if needed).

## Complete Minimal Example

Finally, below is a minimal example combining all the steps into a single program. This example creates a transparent, always-on-top overlay window, runs an event loop to draw an egui UI (with a label and a button), and toggles mouse passthrough automatically. This should be a good starting point or reference for integrating `egui_window_glfw_passthrough` with `glow` in your own application.

```rust
use egui_window_glfw_passthrough::{glfw, GlfwBackend, GlfwConfig};
use egui_glow::Painter;
use glow::Context;
use std::sync::Arc;

fn main() {
    // 1. Configuration for the overlay window
    let config = GlfwConfig {
        width: 800,
        height: 600,
        title: "Overlay Example".to_owned(),
        transparent: true,      // enable alpha channel
        decorated: false,       // no title bar/edges
        always_on_top: true,    // keep above other windows
        resizable: false,       // fixed size (for simplicity)
        ..Default::default()
    };

    // 2. Initialize GlfwBackend and window
    let mut glfw_backend = GlfwBackend::new(config)
        .expect("Failed to create GLFW window backend");
    let window = glfw_backend.window();   // get mutable access to the glfw::Window
    window.make_current();
    window.set_all_polling(true); 
    // The above line enables polling for all types of events (key, mouse, etc.)
    // Alternatively, call specific set_key_polling, set_cursor_pos_polling, etc.

    // 3. Set up OpenGL via glow
    let glow_ctx = unsafe {
        Context::from_loader_function(|s| window.get_proc_address(s) as *const _)
    };
    let glow_ctx = Arc::new(glow_ctx);

    // 4. Create the egui_glow Painter for rendering
    let mut painter = Painter::new(glow_ctx.clone(), "")
        .expect("Failed to initialize egui_glow Painter");

    // 5. Create an egui context for UI state
    let mut egui_ctx = egui::Context::default();

    // Optionally, configure egui visuals for transparent background
    egui_ctx.set_visuals(egui::Visuals::dark()); // use dark theme
    // Make window backgrounds slightly transparent
    egui_ctx.set_style({
        let mut style = (*egui_ctx.style()).clone();
        style.visuals.window_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 30, 180);
        style.visuals.panel_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 30, 180);
        style  // dark gray with 180/255 alpha
    });

    // 6. Main event loop
    let mut last_cursor_pos = egui::pos2(0.0, 0.0);
    loop {
        // Poll events
        glfw_backend.glfw().poll_events();
        // Collect egui events
        let mut egui_events = Vec::new();
        for (_, event) in glfw::flush_messages(&glfw_backend.events) {
            match event {
                glfw::WindowEvent::Close => {
                    // Window closed (e.g., via Alt+F4)
                    return;
                }
                glfw::WindowEvent::Key(key, _scancode, action, mods) => {
                    if let Some(egui_key) = egui_window_glfw_passthrough::glfw_to_egui_key(key) {
                        if let Some(pressed) = egui_window_glfw_passthrough::glfw_to_egui_action(action) {
                            let egui_mods = egui_window_glfw_passthrough::glfw_to_egui_modifiers(mods);
                            egui_events.push(egui::Event::Key { 
                                key: egui_key, pressed, modifiers: egui_mods 
                            });
                        }
                    }
                    // Also handle Escape to close the overlay (optional):
                    if key == glfw::Key::Escape && action == glfw::Action::Press {
                        return;
                    }
                }
                glfw::WindowEvent::Char(ch) => {
                    // Character input for text
                    egui_events.push(egui::Event::Text(ch.to_string()));
                }
                glfw::WindowEvent::MouseButton(btn, action, mods) => {
                    if let Some(egui_btn) = egui_window_glfw_passthrough::glfw_to_egui_pointer_button(btn) {
                        let pressed = action == glfw::Action::Press;
                        let egui_mods = egui_window_glfw_passthrough::glfw_to_egui_modifiers(mods);
                        egui_events.push(egui::Event::PointerButton {
                            pos: last_cursor_pos,
                            button: egui_btn,
                            pressed,
                            modifiers: egui_mods
                        });
                    }
                }
                glfw::WindowEvent::CursorPos(x, y) => {
                    last_cursor_pos = egui::pos2(x as f32, y as f32);
                    egui_events.push(egui::Event::PointerMoved(last_cursor_pos));
                }
                glfw::WindowEvent::CursorEnter(false) => {
                    egui_events.push(egui::Event::PointerGone);
                }
                glfw::WindowEvent::Scroll(_x, y) => {
                    // Convert scroll to egui (y scroll typically)
                    egui_events.push(egui::Event::Scroll(egui::vec2(0.0, y as f32 * 20.0)));
                }
                _ => {}
            }
        }

        // If window was instructed to close (e.g., by user or system), exit loop
        if window.should_close() {
            break;
        }

        // Begin new egui frame with collected input
        let (win_w, win_h) = window.get_size();
        let (fb_w, fb_h) = window.get_framebuffer_size();
        let pixels_per_point = fb_w as f32 / win_w.max(1) as f32;
        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::new(0.0, 0.0),
                egui::vec2(win_w as f32, win_h as f32),
            )),
            pixels_per_point: Some(pixels_per_point),
            // You can set also .time, .predicted_dt, .modifiers if needed
            ..Default::default()
        };
        raw_input.events = egui_events;
        egui_ctx.begin_frame(raw_input);

        // Build UI (example)
        egui::TopBottomPanel::top("top_panel").show(&egui_ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("🔵 Overlay HUD");
                if ui.button("Show/Hide").clicked() {
                    // In a real app, toggle overlay visibility
                    // (Here we might close or hide the overlay window)
                }
            });
        });
        egui::Window::new("info_window")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]) // center of screen
            .resizable(false)
            .title_bar(false)
            .show(&egui_ctx, |ui| {
                ui.label("This is an egui overlay window.");
                ui.label("Click the button below or press Esc to exit.");
                if ui.button("Exit Overlay").clicked() {
                    window.set_should_close(true);
                }
            });

        // End UI frame and get draw commands
        let full_output = egui_ctx.end_frame();
        let paint_jobs = full_output.shapes;
        let textures_delta = full_output.textures_delta;
        let clipped_primitives = egui_ctx.tessellate(paint_jobs);

        // Configure OpenGL for new frame (clear to transparent)
        unsafe {
            // Clear the framebuffer with transparent color
            glow_ctx.clear_color(0.0, 0.0, 0.0, 0.0);
            glow_ctx.clear(glow::COLOR_BUFFER_BIT);
        }

        // Paint the egui UI using the glow Painter
        painter.paint_and_update_textures(
            [fb_w as u32, fb_h as u32],
            pixels_per_point,
            &clipped_primitives,
            &textures_delta,
        ).unwrap();

        // If egui requested to quit the app (e.g., via `ctx.output().close`), break
        if full_output.platform_output.closed {
            break;
        }
        // Handle copy-text output or cursor icons if needed via full_output.platform_output

        // Toggle mouse passthrough based on egui's needs:
        let wants_input = egui_ctx.wants_pointer_input() || egui_ctx.wants_keyboard_input();
        window.set_mouse_passthrough(!wants_input);

        // Display the frame
        window.swap_buffers();
    }
}
```

**Explanation of the example:** This program opens an overlay window with specified settings. It polls events in a loop and uses the conversion helpers to build a list of `egui::Event`. It then sets up `RawInput` each frame including the current window size and scale for HiDPI, and begins an egui frame. The UI in the example shows a top panel with a title and a dummy "Show/Hide" button, and a centered window with some text and an "Exit Overlay" button. The egui frame is ended, producing shapes and texture updates which are fed into the `egui_glow::Painter`. We clear the screen to transparent each frame to avoid ghosting previous frame’s content. After drawing, we check if egui requested the window to close (`platform_output.closed` flag) and also demonstrate handling of other platform output if needed (like copying text to clipboard or changing cursor icon, which one could forward to the system or GLFW as needed). Finally, we set the mouse passthrough: if egui doesn’t need input, we enable passthrough so that the overlay doesn’t block clicks. Then we swap buffers to show the frame.

When you run this example on a Linux X11 system (with a compositing window manager that supports transparency), you should see a window with the UI text, and you should be able to see underlying windows behind any transparent parts. Clicking on the overlay’s button will trigger the printed message or exit, and when your cursor is not hovering the UI, you can click “through” the overlay to interact with windows behind it. Pressing Escape or the "Exit Overlay" button will close the overlay window and end the program.

## Additional Notes

* **Always-on-top and Focus:** The `always_on_top` (floating) hint ensures the overlay stays above other normal windows. Keep in mind that on some platforms (like Wayland) this might not be honored; on X11 it works as expected. The overlay window can be completely interaction-transparent when passthrough is on, but it will still remain on top visually. If you need the overlay to hide or show, you can call `window.hide()` or `window.show()` accordingly or even close and recreate it.
* **Performance:** Egui is quite efficient for moderate UI. If your overlay is simple (few widgets), CPU usage will be low especially with vsync limiting the frame rate. If you don’t need continuous updates, you could even throttle the loop or only redraw on demand (but with an overlay, often continuous redraw is fine for responsiveness, e.g., updating a clock or FPS counter).
* **No Compositor / Transparency issues:** On X11, a compositing window manager (e.g., picom, KWin, GNOME Shell) is required for transparency to actually display. If no compositor is present, the transparent parts may appear black. This is a system limitation, not a code issue. Since this guide assumes a typical Linux desktop with Xorg, we expect a compositor to be running by default.
* **Blur and other effects:** The crate and this setup do not handle blur behind the window or other advanced effects – those would be up to the window manager. The overlay simply provides transparency and passthrough.
* **Compatibility:** `egui_window_glfw_passthrough` supports Linux and Windows (and partially macOS). This example is tailored to Linux/X11. On Windows, the transparency and passthrough should also work (GLFW uses layered windows and extended styles there), but the specifics of DPI handling might differ slightly (GLFW should abstract it similarly). MacOS doesn’t support transparent OpenGL layers easily (since OpenGL is deprecated there), but the crate has fallbacks (using metal or wgpu if chosen). If targeting Mac, consider using the wgpu backend instead of glow for full support.

This documentation should serve as a comprehensive reference to implement your own always-on-top transparent overlay with egui and GLFW. By following the example and adjusting the UI code, you can create a custom HUD or overlay. Just remember to toggle `set_mouse_passthrough` as needed to allow users to interact with background windows when the overlay is idle. Happy coding!




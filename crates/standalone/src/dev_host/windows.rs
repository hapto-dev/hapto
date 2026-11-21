//! The dev host's windows: the main one, the bar across its top and the
//! plugin's view below, and the settings window beside it, which a close
//! hides, so it opens again as it was.

use anyhow::{Context as _, Result};
use hapto_host::vst3::ChildWindow;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use super::layout::{self, Rect};
use super::panel::Panel;
use super::{DevHost, PageState, SettingsWindow};
use crate::app::{EditorShown, minimized};

impl DevHost {
    /// Opens the main window: the plugin's view, sized as it asks, below the
    /// bar. With no view, the bar fills it, and the settings window opens
    /// too, since it holds the plugin's parameters.
    pub(super) fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let (width, height) = layout::NO_EDITOR;
        let attributes = Window::default_attributes()
            .with_title(format!("{} - dev host", self.about.name))
            .with_inner_size(LogicalSize::new(width, height))
            .with_visible(false);
        let window = event_loop.create_window(attributes).context("the window")?;
        let scale = window.scale_factor();
        let mut view = self.hosted.view();
        if let Some(view) = &view {
            let _scaled = view.set_scale(scale as f32);
            // A view that cannot resize fixes the window's size, so no part
            // of the window is left uncovered beside or below it.
            window.set_resizable(view.can_resize());
            if let Some((width, height)) = view.size() {
                fit(&window, width, height);
            }
        }
        let size = logical(window.inner_size(), scale);
        let mut placed = layout::lay_out(size.width, size.height, view.is_some());
        self.editor = if view.is_some() {
            EditorShown::Open
        } else {
            EditorShown::None
        };
        if let (Some(shown), Some(rect)) = (&view, placed.view) {
            let (x, y, width, height) = rect.pixels(scale);
            let frame = ChildWindow::new(&window, x, y, width, height)
                .context("the system refused the view's window")?;
            match shown.attach_to(&frame) {
                Ok(()) => self.frame = Some(frame),
                // As if the plugin had none, so the bar says so, the window
                // is laid out without a view, and the settings window opens.
                Err(error) => {
                    eprintln!("{}: the editor did not open: {error}", self.about.name);
                    view = None;
                    drop(frame);
                    window.set_resizable(true);
                    placed = layout::lay_out(size.width, size.height, false);
                    self.editor = EditorShown::Failed;
                }
            }
        }
        let page = include_str!("bar.html");
        let bar = Panel::open(&window, page, placed.bar, self.session.path("bar"))?;
        self.bar = Some(bar);
        window.set_visible(true);
        let has_view = view.is_some();
        self.view = view;
        self.window = Some(window);
        if !has_view {
            self.open_settings(event_loop)?;
        }
        Ok(())
    }

    /// Opens the settings window beside the main one, or shows it and
    /// brings it to the front when it is open already.
    pub(super) fn open_settings(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if let Some(settings) = &self.settings {
            settings.window.set_visible(true);
            settings.window.focus_window();
            return Ok(());
        }
        let (width, height) = layout::SETTINGS;
        let mut attributes = Window::default_attributes()
            .with_title(format!("{} - dev host settings", self.about.name))
            .with_inner_size(LogicalSize::new(width, height));
        if let Some(main) = &self.window
            && let Ok(at) = main.outer_position()
        {
            let right = i32::try_from(main.outer_size().width).unwrap_or(0);
            attributes = attributes.with_position(PhysicalPosition::new(at.x + right, at.y));
        }
        let window = event_loop
            .create_window(attributes)
            .context("the settings window")?;
        let size = logical(window.inner_size(), window.scale_factor());
        let page = include_str!("settings.html");
        let rect = Rect::whole(size.width, size.height);
        let panel = Panel::open(&window, page, rect, self.session.path("settings"))?;
        self.settings = Some(SettingsWindow {
            panel,
            window,
            page: PageState::Loading,
        });
        Ok(())
    }

    /// Whether `id` is the settings window's.
    pub(super) fn is_settings(&self, id: WindowId) -> bool {
        self.settings.as_ref().is_some_and(|s| s.window.id() == id)
    }

    /// Handles an event of the settings window: a close hides it, and a
    /// resize fills it with its page again.
    pub(super) fn settings_event(&self, event: WindowEvent) {
        let Some(settings) = &self.settings else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => settings.window.set_visible(false),
            WindowEvent::Resized(size) => {
                let size = logical(size, settings.window.scale_factor());
                settings.panel.place(Rect::whole(size.width, size.height));
            }
            _ => {}
        }
    }

    /// Lays the main window out for its new `size`: the bar across the top,
    /// and the view, in its window, below it, resized where it resizes.
    pub(super) fn resized(&self, size: PhysicalSize<u32>) {
        let Some(window) = self.window.as_ref().filter(|_| !minimized(size)) else {
            return;
        };
        let scale = window.scale_factor();
        let size = logical(size, scale);
        let placed = layout::lay_out(size.width, size.height, self.view.is_some());
        if let Some(bar) = &self.bar {
            bar.place(placed.bar);
        }
        if let (Some(frame), Some(rect)) = (&self.frame, placed.view) {
            let (x, y, width, height) = rect.pixels(scale);
            frame.place(x, y, width, height);
            if let Some(view) = self.view.as_ref().filter(|view| view.can_resize()) {
                let _resized = view.resize(width, height);
            }
        }
    }

    /// Resizes the main window for a size the view asked its frame for, as a
    /// host does.
    pub(super) fn follow_the_view(&self) {
        let (Some(view), Some(window)) = (&self.view, &self.window) else {
            return;
        };
        if let Some(&(width, height)) = view.resizes().last() {
            fit(window, width, height);
        }
    }
}

/// Sizes `window` for a view of `width` by `height` in the screen's pixels,
/// with the bar above it.
fn fit(window: &Window, width: i32, height: i32) {
    let scale = window.scale_factor();
    let to_logical = |pixels: i32| (f64::from(pixels.max(0)) / scale).round() as u32;
    let (width, height) = layout::window_for(to_logical(width), to_logical(height));
    let _resized = window.request_inner_size(LogicalSize::new(width, height));
}

fn logical(size: PhysicalSize<u32>, scale: f64) -> LogicalSize<u32> {
    size.to_logical(scale)
}

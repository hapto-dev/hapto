//! The editor's view: the window a host shows the plugin's editor in, as
//! VST3's `IPlugView`, with `IPlugViewContentScaleSupport` for the screen's
//! scale.
//!
//! A view keeps a reference to its component, as the SDK's own views do: a
//! host that releases the component before the view, or never removes the
//! view, leaves nothing dangling, since the component and its engine live
//! until the view goes. Every call is guarded as the component's are, and
//! refused on any thread but the plugin's main thread, which counts as a
//! fault. The editor is driven through the engine's [`EditorSlot`], which
//! puts off what a host asks from inside another of the editor's calls.
//!
//! On Windows a view's size is in the screen's pixels, and an editor's in
//! logical ones, so the view converts with the scale the host last gave. A
//! scale that changes while the editor is open asks the host's frame for the
//! size the editor now needs. Only Windows' `HWND` is a platform a view
//! attaches to, and the exporter makes no view anywhere else.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicPtr, Ordering};

use ::vst3::Steinberg::IPlugViewContentScaleSupport_::ScaleFactor;
use ::vst3::Steinberg::Vst::IEditController;
use ::vst3::Steinberg::{
    FIDString, IPlugFrame, IPlugFrameTrait, IPlugView, IPlugViewContentScaleSupport,
    IPlugViewContentScaleSupportTrait, IPlugViewTrait, TBool, ViewRect, char16, int16,
    kInternalError, kInvalidArgument, kResultFalse, kResultOk, kResultTrue, tresult,
};
use ::vst3::com_scrape_types::Unknown;
use ::vst3::{Class, ComPtr, ComRef, ComWrapper};

use super::component::Handler;
use crate::boundary::MainThread;
use crate::engine::{EditorSlot, Engine, Opened};
use crate::{Plugin, Size};

/// The platform type the view attaches to, as the SDK names it.
const HWND: &CStr = c"HWND";

/// One view of the plugin's editor.
pub(crate) struct View<P: Plugin> {
    /// Dropped first, so the frame goes before the component may; the
    /// editor goes earlier still, in `drop`.
    state: MainThread<ViewState>,
    /// The component's engine, which `component` keeps alive.
    engine: NonNull<Engine<P, Handler>>,
    /// A reference to the component, released last.
    component: ComponentRef,
}

/// A view's reference to its component: released as the view drops, or at
/// the end of a call the host released the view from inside, whichever comes
/// first.
struct ComponentRef(AtomicPtr<IEditController>);

impl ComponentRef {
    fn new(component: ComPtr<IEditController>) -> Self {
        ComponentRef(AtomicPtr::new(component.into_raw()))
    }

    /// Releases the reference, once: a later call does nothing. The
    /// component, and its engine, may go with it.
    fn release(&self) {
        let raw = self.0.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: a pointer that is not null is the reference `new` took,
        // swapped out here once, so it is released once.
        drop(unsafe { ComPtr::from_raw(raw) });
    }
}

impl Drop for ComponentRef {
    fn drop(&mut self) {
        self.release();
    }
}

/// What only the plugin's main thread touches.
struct ViewState {
    /// The editor's slot, taken as the view drops, so that the editor drops
    /// inside the view's guard: a panic in its drop must not reach the host.
    slot: RefCell<Option<Rc<EditorSlot>>>,
    frame: RefCell<Option<ComPtr<IPlugFrame>>>,
    /// This view as the host holds it, without a reference of its own:
    /// what a resize request names, and what a call holds the view by.
    this: Cell<*mut IPlugView>,
}

impl<P: Plugin> Class for View<P> {
    type Interfaces = (IPlugView, IPlugViewContentScaleSupport);
}

/// Makes a view of `slot`'s editor that holds `component`, whose engine is
/// `engine`, and hands the host one reference to it.
///
/// Thread: main, which the view is bound to.
pub(crate) fn create<P: Plugin>(
    component: ComPtr<IEditController>,
    engine: &Engine<P, Handler>,
    slot: Rc<EditorSlot>,
) -> *mut IPlugView {
    let view = ComWrapper::new(View {
        state: MainThread::bound(ViewState {
            slot: RefCell::new(Some(slot)),
            frame: RefCell::new(None),
            this: Cell::new(std::ptr::null_mut()),
        }),
        engine: NonNull::from(engine),
        component: ComponentRef::new(component),
    });
    let Some(this) = view.as_com_ref::<IPlugView>() else {
        return std::ptr::null_mut();
    };
    if let Ok(state) = view.state.lend() {
        state.this.set(this.as_ptr());
    }
    view.to_com_ptr::<IPlugView>()
        .map_or(std::ptr::null_mut(), ComPtr::into_raw)
}

impl<P: Plugin> View<P> {
    fn engine(&self) -> &Engine<P, Handler> {
        // SAFETY: `component` holds a reference to the component, whose
        // `Drop` alone frees the engine, so the engine lives while `self`
        // does; it is only ever shared.
        unsafe { self.engine.as_ref() }
    }

    /// Runs a call's body inside the instance's guard, on its main thread
    /// only, with the view's state and the editor's slot: answers `refused`
    /// on another thread, which counts as a fault, or when the body panics.
    fn call(
        &self,
        refused: tresult,
        body: impl FnOnce(&ViewState, &EditorSlot) -> tresult,
    ) -> tresult {
        let engine = self.engine();
        engine.guard(refused, || {
            let Some(_main) = engine.main() else {
                return refused;
            };
            let Ok(state) = self.state.lend() else {
                return refused;
            };
            // Cloned out, so no borrow is held while the editor or the host
            // runs. It drops inside the guard, and the editor may with it.
            let Some(slot) = state.slot.borrow().clone() else {
                return refused;
            };
            body(&state, &slot)
        })
    }

    /// As [`call`](Self::call), holding a reference to the view for the
    /// body's length: for a call whose body may reach the host, which may
    /// release the view from inside it. The reference goes after the guard
    /// and the main thread's loan have ended.
    ///
    /// A host that released every reference of its own meanwhile, though
    /// COM has a caller hold what it calls across the call, leaves this one
    /// the last. Dropping it would free the view under its own call, which
    /// Rust's aliasing rules forbid, so the view goes as far as it can
    /// instead: its editor, its frame and its component now, and its own
    /// few bytes never.
    fn call_held(
        &self,
        refused: tresult,
        body: impl FnOnce(&ViewState, &EditorSlot) -> tresult,
    ) -> tresult {
        let mut held = None;
        let answer = self.call(refused, |state, slot| {
            // SAFETY: `this` is this view's own interface, set as it was
            // made, and this is one of its calls, so it is alive.
            // `to_com_ptr` adds the reference `held` keeps.
            held = unsafe { ComRef::from_raw(state.this.get()) }.map(|view| view.to_com_ptr());
            body(state, slot)
        });
        let Some(held) = held else {
            return answer;
        };
        if Self::only_holder(&held) {
            self.retire();
            // The component, and the engine with it, may go here: nothing
            // touches either after.
            self.component.release();
            std::mem::forget(held);
        } else {
            drop(held);
        }
        answer
    }

    /// Whether `held` is the only reference to the view: the host released
    /// every one of its own. Counted by adding a reference and taking it
    /// back, which COM answers with the count, on the main thread, where a
    /// host holds its views.
    fn only_holder(held: &ComPtr<IPlugView>) -> bool {
        let this = held.as_ptr();
        // SAFETY: `held` keeps the view alive across both calls, and the
        // reference added is taken back at once.
        let count = unsafe {
            let count = IPlugView::add_ref(this);
            IPlugView::release(this);
            count
        };
        count == 2
    }

    /// Closes the editor and lets it go, with the frame, on the main thread
    /// and inside the guard, unless a tick still holds the editor.
    /// Elsewhere the state leaks, as bound state does, and the call is a
    /// fault.
    fn retire(&self) {
        self.call(kResultOk, |state, slot| {
            // Guarded on its own, so a panic in the editor's `close` ends
            // here and the slot is still taken: the editor then drops as
            // this call returns, inside its guard, and not while that panic
            // unwinds, where a panic in its drop would abort the host.
            self.engine().guard((), || slot.close());
            drop(state.slot.take());
            drop(state.frame.take());
            kResultOk
        });
    }

    /// The view's size in the screen's pixels, for `size` at `scale`.
    fn rect(size: Size, scale: f64) -> ViewRect {
        let pixels = |logical: u32| (f64::from(logical) * scale).round() as i32;
        ViewRect {
            left: 0,
            top: 0,
            right: pixels(size.width),
            bottom: pixels(size.height),
        }
    }

    /// The editor's size for a view of `rect` in the screen's pixels at
    /// `scale`.
    fn size(rect: &ViewRect, scale: f64) -> Size {
        let logical = |pixels: i32| {
            (f64::from(pixels.max(0)) / scale)
                .round()
                .clamp(0.0, f64::from(u32::MAX))
        };
        Size {
            width: logical(rect.right.saturating_sub(rect.left)) as u32,
            height: logical(rect.bottom.saturating_sub(rect.top)) as u32,
        }
    }

    /// Asks the host's frame to resize the view to the editor's size at the
    /// scale given last, as when the scale changed.
    fn ask_to_resize(state: &ViewState, slot: &EditorSlot) {
        // Cloned out, so no borrow is held while the host calls back in.
        let frame = state.frame.borrow().clone();
        if let Some(frame) = frame {
            let mut rect = Self::rect(slot.size(), slot.scale());
            // SAFETY: `frame` holds a reference to the host's frame; `this`
            // is this view, alive for this call; `rect` is valid for the
            // call. The host may call `onSize` from inside, which borrows
            // nothing this call holds.
            unsafe { frame.resizeView(state.this.get(), &mut rect) };
        }
    }
}

impl<P: Plugin> Drop for View<P> {
    fn drop(&mut self) {
        // A view the host releases without removing it closes its editor
        // here; the component goes last, with the field.
        self.retire();
    }
}

impl<P: Plugin> IPlugViewTrait for View<P> {
    unsafe fn isPlatformTypeSupported(&self, r#type: FIDString) -> tresult {
        self.call(kResultFalse, |_, _| {
            // SAFETY: the SDK's contract: `type` is null or a NUL-terminated
            // string valid during the call.
            if cfg!(windows) && !r#type.is_null() && unsafe { CStr::from_ptr(r#type) } == HWND {
                kResultTrue
            } else {
                kResultFalse
            }
        })
    }

    unsafe fn attached(&self, parent: *mut c_void, r#type: FIDString) -> tresult {
        // Held: the editor's `open` may run the message loop, and the host
        // may release the view inside it.
        self.call_held(kResultFalse, |_, slot| {
            // SAFETY: as in `isPlatformTypeSupported`.
            if r#type.is_null() || unsafe { CStr::from_ptr(r#type) } != HWND {
                return kResultFalse;
            }
            let Some(parent) = parent_window(parent) else {
                return kInvalidArgument;
            };
            match slot.open(&parent) {
                Opened::Yes => kResultOk,
                Opened::Already => kResultFalse,
                Opened::Busy => {
                    self.engine().editor_failed(
                        &"the host attached the view again while the editor was still \
                          in a call, so it stays closed until the next attach",
                    );
                    kResultFalse
                }
                Opened::Failed(error) => {
                    self.engine().editor_failed(&error);
                    kResultFalse
                }
                Opened::Panicked => {
                    self.engine().editor_panicked();
                    kResultFalse
                }
            }
        })
    }

    unsafe fn removed(&self) -> tresult {
        self.call_held(kResultFalse, |_, slot| {
            slot.close();
            kResultOk
        })
    }

    unsafe fn onWheel(&self, _distance: f32) -> tresult {
        kResultFalse
    }

    unsafe fn onKeyDown(&self, _key: char16, _key_code: int16, _modifiers: int16) -> tresult {
        // Not handled: an editor's own window takes its keys from the
        // system, and the host keeps whatever reaches it.
        kResultFalse
    }

    unsafe fn onKeyUp(&self, _key: char16, _key_code: int16, _modifiers: int16) -> tresult {
        kResultFalse
    }

    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        self.call(kInternalError, |_, slot| {
            // SAFETY: the SDK's contract: `size` is null or valid for writing
            // one `ViewRect` until the call returns.
            let Some(size) = (unsafe { size.as_mut() }) else {
                return kInvalidArgument;
            };
            *size = Self::rect(slot.size(), slot.scale());
            kResultOk
        })
    }

    unsafe fn onSize(&self, new_size: *mut ViewRect) -> tresult {
        self.call_held(kInternalError, |_, slot| {
            // SAFETY: the SDK's contract: `new_size` is null or valid for
            // reading one `ViewRect` until the call returns.
            let Some(rect) = (unsafe { new_size.as_ref() }) else {
                return kInvalidArgument;
            };
            slot.set_size(Self::size(rect, slot.scale()));
            kResultOk
        })
    }

    unsafe fn onFocus(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setFrame(&self, frame: *mut IPlugFrame) -> tresult {
        self.call(kInternalError, |state, _| {
            // SAFETY: the SDK's contract: `frame` is null or the host's live
            // frame during the call; `to_com_ptr` adds a reference, which
            // keeps it alive until the next `setFrame` or the view's drop.
            let frame = unsafe { ComRef::from_raw(frame) }.map(|frame| frame.to_com_ptr());
            // Taken out first, so a frame's release runs with no borrow held.
            let old = state.frame.replace(frame);
            drop(old);
            kResultOk
        })
    }

    unsafe fn canResize(&self) -> tresult {
        kResultTrue
    }

    unsafe fn checkSizeConstraint(&self, rect: *mut ViewRect) -> tresult {
        self.call(kInternalError, |_, _| {
            // Any size the host offers is taken as it is.
            if rect.is_null() {
                kInvalidArgument
            } else {
                kResultTrue
            }
        })
    }
}

impl<P: Plugin> IPlugViewContentScaleSupportTrait for View<P> {
    unsafe fn setContentScaleFactor(&self, factor: ScaleFactor) -> tresult {
        self.call_held(kInternalError, |state, slot| {
            let scale = f64::from(factor);
            if !(scale.is_finite() && scale > 0.0) {
                return kInvalidArgument;
            }
            if slot.set_scale(scale) && slot.is_open() {
                Self::ask_to_resize(state, slot);
            }
            kResultOk
        })
    }
}

/// The window a host handed `attached`, as an editor takes it; `None` for
/// null, and on any platform but Windows.
fn parent_window(parent: *mut c_void) -> Option<crate::ParentWindow> {
    #[cfg(windows)]
    {
        use std::num::NonZeroIsize;

        use raw_window_handle::{RawWindowHandle, Win32WindowHandle};

        let hwnd = NonZeroIsize::new(parent as isize)?;
        Some(crate::ParentWindow::new(RawWindowHandle::Win32(
            Win32WindowHandle::new(hwnd),
        )))
    }
    #[cfg(not(windows))]
    {
        let _ = parent;
        None
    }
}

impl<P: Plugin> std::fmt::Debug for View<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("plugin", &P::INFO.name)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, windows))]
mod tests;

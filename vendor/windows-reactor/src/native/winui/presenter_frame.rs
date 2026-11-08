//! Native AppWindow frame state for a composed Mica card.

use super::bindings::{IWindow2, Window};
use std::ffi::c_void;
use windows_core::{IInspectable_Vtbl, Interface};

/// Holds the presenter settings for the lifetime of a frameless Mica card.
pub(super) struct PresenterFrame {
    presenter: IOverlappedPresenterFrame,
    original: Option<(bool, bool)>,
}

impl PresenterFrame {
    pub(super) fn hide(window: &Window) -> windows_core::Result<Self> {
        let presenter = window
            .cast::<IWindow2>()?
            .AppWindow()?
            .Presenter()?
            .cast::<IOverlappedPresenterFrame>()?;
        let original = (presenter.has_border()?, presenter.has_title_bar()?);
        presenter.set_border_and_title_bar(false, false)?;
        Ok(Self {
            presenter,
            original: Some(original),
        })
    }

    pub(super) fn restore(&mut self) -> windows_core::Result<()> {
        if let Some((border, title_bar)) = self.original {
            self.presenter.set_border_and_title_bar(border, title_bar)?;
            self.original = None;
        }
        Ok(())
    }
}

impl Drop for PresenterFrame {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

// Public IOverlappedPresenter IID and method order verified from Microsoft.UI.winmd.
// HasBorder/HasTitleBar are slots 0/1; SetBorderAndTitleBar is slot 16.
windows_core::imp::define_interface!(
    IOverlappedPresenterFrame,
    IOverlappedPresenterFrameVtbl,
    0x21693970_4f4c_5172_9e9d_682a2d174884
);
#[repr(C)]
pub struct IOverlappedPresenterFrameVtbl {
    base: IInspectable_Vtbl,
    has_border: unsafe extern "system" fn(*mut c_void, *mut bool) -> windows_core::HRESULT,
    has_title_bar: unsafe extern "system" fn(*mut c_void, *mut bool) -> windows_core::HRESULT,
    before_set_frame: [usize; 14],
    set_border_and_title_bar:
        unsafe extern "system" fn(*mut c_void, bool, bool) -> windows_core::HRESULT,
}

impl IOverlappedPresenterFrame {
    fn has_border(&self) -> windows_core::Result<bool> {
        unsafe {
            let mut value = false;
            (self.vtable().has_border)(self.as_raw(), &mut value).map(|| value)
        }
    }

    fn has_title_bar(&self) -> windows_core::Result<bool> {
        unsafe {
            let mut value = false;
            (self.vtable().has_title_bar)(self.as_raw(), &mut value).map(|| value)
        }
    }

    fn set_border_and_title_bar(&self, border: bool, title_bar: bool) -> windows_core::Result<()> {
        unsafe { (self.vtable().set_border_and_title_bar)(self.as_raw(), border, title_bar).ok() }
    }
}

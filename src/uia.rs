//! Reads the text of a browser's address bar through UI Automation, the
//! accessibility interface every Windows program offers to tools.
//!
//! Only the browser's own toolbar is looked at: page content (the document)
//! is never walked, which keeps the cost small and avoids switching the
//! browser into its heavier accessibility mode for web pages. The caller
//! turns the text into a domain and throws the rest away.

use std::collections::HashMap;
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ValuePatternId,
};

/// How many toolbar elements may be visited while looking for the address bar.
const NODE_BUDGET: u32 = 600;
const MAX_DEPTH: u32 = 14;
/// Milliseconds a browser may take to answer before the call is abandoned.
const TIMEOUT_MS: u32 = 400;

pub struct AddressBar {
    automation: IUIAutomation,
    walker: IUIAutomationTreeWalker,
    /// The address-bar element found for a window, so later reads are one call.
    cache: HashMap<isize, IUIAutomationElement>,
}

impl AddressBar {
    /// Must be called on the thread that will use it (it initialises COM
    /// there). `None` when UI Automation is unavailable.
    pub fn new() -> Option<AddressBar> {
        // SAFETY: standard COM initialisation and object creation on this thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            if let Ok(a2) = automation.cast::<IUIAutomation2>() {
                // A hung browser must not hang the tracker.
                let _ = a2.SetConnectionTimeout(TIMEOUT_MS);
                let _ = a2.SetTransactionTimeout(TIMEOUT_MS);
            }
            let walker = automation.ControlViewWalker().ok()?;
            Some(AddressBar {
                automation,
                walker,
                cache: HashMap::new(),
            })
        }
    }

    /// The address-bar text of a browser window, or `None` when it cannot be
    /// found or the user is typing in it right now (then it holds what is
    /// being typed, not where the page is).
    pub fn read(&mut self, hwnd: isize) -> Option<String> {
        if let Some(element) = self.cache.get(&hwnd) {
            match value_of(element) {
                Ok(v) => return v,
                Err(()) => {
                    self.cache.remove(&hwnd);
                }
            }
        }
        if self.cache.len() > 64 {
            self.cache.clear();
        }
        // SAFETY: COM calls on interfaces owned by this object, on its thread.
        let element = unsafe {
            let root = self
                .automation
                .ElementFromHandle(HWND(hwnd as *mut c_void))
                .ok()?;
            let mut budget = NODE_BUDGET;
            self.find_edit(&root, 0, &mut budget)?
        };
        let value = value_of(&element).ok()?;
        self.cache.insert(hwnd, element);
        value
    }

    /// Forgets a window (it was closed).
    pub fn forget(&mut self, hwnd: isize) {
        self.cache.remove(&hwnd);
    }

    /// Depth-first search for the first edit field outside page content.
    unsafe fn find_edit(
        &self,
        parent: &IUIAutomationElement,
        depth: u32,
        budget: &mut u32,
    ) -> Option<IUIAutomationElement> {
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut child = self.walker.GetFirstChildElement(parent).ok();
        while let Some(element) = child {
            if *budget == 0 {
                return None;
            }
            *budget -= 1;
            let kind = element.CurrentControlType().ok()?;
            if kind == UIA_EditControlTypeId {
                return Some(element);
            }
            // Never descend into the web page.
            if kind != UIA_DocumentControlTypeId {
                if let Some(found) = self.find_edit(&element, depth + 1, budget) {
                    return Some(found);
                }
            }
            child = self.walker.GetNextSiblingElement(&element).ok();
        }
        None
    }
}

/// `Err` = the element is stale; `Ok(None)` = fine but not usable right now.
fn value_of(element: &IUIAutomationElement) -> Result<Option<String>, ()> {
    // SAFETY: COM calls on a live interface pointer.
    unsafe {
        let focused = element.CurrentHasKeyboardFocus().map_err(|_| ())?;
        let pattern: IUIAutomationValuePattern = element
            .GetCurrentPatternAs(UIA_ValuePatternId)
            .map_err(|_| ())?;
        let value = pattern.CurrentValue().map_err(|_| ())?.to_string();
        if focused.as_bool() || value.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(value))
    }
}

//! WKWebView turns down the Pointer Lock API unless its UI delegate agrees, through a private method wry does not have.
//! Without it the game falls back to dragging to look around. This adds that method to wry's delegate class and grants every request.

use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::sel;
use objc2_web_kit::WKWebView;

/// `_webViewDidRequestPointerLock:completionHandler:` always says yes (Esc still releases the pointer).
unsafe extern "C-unwind" fn grant(
    _this: *mut AnyObject,
    _cmd: Sel,
    _webview: *mut AnyObject,
    handler: *mut block2::Block<dyn Fn(Bool)>,
) {
    // SAFETY: WebKit passes a valid completion block (or null, which `as_ref` turns into None).
    if let Some(h) = unsafe { handler.as_ref() } {
        h.call((Bool::YES,));
    }
}

pub fn enable(wv: &tauri::Webview) {
    let _ = wv.with_webview(|pv| {
        // SAFETY: on macOS `inner()` is the WKWebView, alive for the duration of this closure.
        let wk = unsafe { &*(pv.inner() as *const WKWebView) };
        let Some(delegate) = (unsafe { wk.UIDelegate() }) else { return };
        let cls: &AnyClass = AsRef::<AnyObject>::as_ref(&*delegate).class();
        let s = sel!(_webViewDidRequestPointerLock:completionHandler:);
        if !cls.responds_to(s) {
            // SAFETY: `grant` matches the type encoding below (void; self, _cmd, WKWebView*, block).
            unsafe {
                let imp: Imp = std::mem::transmute(
                    grant as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut block2::Block<dyn Fn(Bool)>),
                );
                objc2::ffi::class_addMethod(cls as *const AnyClass as *mut AnyClass, s, imp, c"v@:@@?".as_ptr());
            }
        }
        // WebKit reads which optional methods the delegate has when it is set, so set it again.
        unsafe { wk.setUIDelegate(Some(&delegate)) };
    });
}

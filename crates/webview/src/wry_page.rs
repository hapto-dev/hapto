//! The page in a wry WebView, the part that both engines build the same way:
//! the scheme that serves the page's files, and the checks that keep the
//! WebView on the page. WebView2 builds it in the plugin's process, and
//! WebKitGTK in the page's helper process.

use std::borrow::Cow;
use std::panic::{self, AssertUnwindSafe};

use wry::http::header::{CONTENT_TYPE, HeaderValue};
use wry::http::{Response, StatusCode};
use wry::{WebContext, WebViewBuilder};

use crate::Asset;
use crate::assets::{SCHEME, Served, is_page, page_url, serve};

/// Returns a builder for a WebView that shows the page: the dev server's
/// page when `dev` names one, or else the page built into the plugin, served
/// from `assets` at the page's scheme, which the engine serves at `origin`.
/// The WebView passes each message that the page posts to `receive`.
///
/// A context serves a scheme for every WebView in it, and wry refuses to
/// register one twice. So in a context that serves the scheme already, as
/// the helper's does after its first page, the WebView gets the files of the
/// page that registered it, which are the same plugin's.
pub(crate) fn builder<'a>(
    context: &'a mut WebContext,
    origin: &'static str,
    dev: Option<String>,
    assets: &'static [Asset],
    receive: impl Fn(String) + 'static,
) -> WebViewBuilder<'a> {
    let navigable = dev.clone();
    let registered = context.is_custom_protocol_registered(SCHEME);
    let builder = WebViewBuilder::new_with_web_context(context).with_url(page_url(dev.as_deref()));
    let builder = if registered {
        builder
    } else {
        builder.with_custom_protocol(SCHEME.to_owned(), move |_, request| {
            respond(assets, request.uri().path())
        })
    };
    builder
        .with_ipc_handler(move |request| {
            // Only the page speaks for the user, so any other document's
            // message is dropped.
            if is_page(&request.uri().to_string(), origin, dev.as_deref()) {
                receive(request.into_body());
            }
        })
        // The WebView shows the page and nothing else, so a link or a script
        // can't take it elsewhere.
        .with_navigation_handler(move |url| is_page(&url, origin, navigable.as_deref()))
        // The page takes the keyboard when it is clicked, not as it opens, so
        // the host keeps it until then.
        .with_focused(false)
}

/// Returns the response to a request for `path`, which borrows the file's
/// bytes from `assets`.
fn respond(assets: &'static [Asset], path: &str) -> Response<Cow<'static, [u8]>> {
    answer(|| serve(assets, path))
}

/// The response to a request whose serving panicked.
const FAILED: Served = Served {
    status: 500,
    content_type: "text/plain; charset=utf-8",
    body: b"the file could not be served",
};

/// Returns the response for what `serve` returns, or a 500 response if it
/// panics, because it runs inside the WebView's callback, where a panic must
/// not unwind.
fn answer(serve: impl FnOnce() -> Served) -> Response<Cow<'static, [u8]>> {
    let served = panic::catch_unwind(AssertUnwindSafe(serve)).unwrap_or(FAILED);
    let mut response = Response::new(Cow::Borrowed(served.body));
    *response.status_mut() = StatusCode::from_u16(served.status).unwrap_or(StatusCode::NOT_FOUND);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(served.content_type));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::PAGE;

    #[test]
    fn a_file_is_served_and_a_panic_while_serving_answers_500() {
        let served = respond(PAGE, "/index.html");
        assert_eq!(served.status(), StatusCode::OK);
        assert_eq!(served.body().as_ref(), b"<!doctype html>");
        let failed = answer(|| panic!("a bug while serving"));
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            failed.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/plain; charset=utf-8"))
        );
    }
}

//! The page's files, which are built into the plugin, and how the WebView is
//! served them. It also holds the URL that the WebView loads.

/// The scheme that serves the page built into the plugin. The origin that the
/// page then has differs between engines, and each engine's `ORIGIN` names
/// it.
pub(crate) const SCHEME: &str = "hapto";

/// The URL of the page built into the plugin.
const EMBEDDED: &str = "hapto://localhost/index.html";

/// One file of a page built into the plugin: its path below the page's
/// root, as the page's HTML names it, and its bytes, as `include_bytes!`
/// gives them.
///
/// # Examples
///
/// ```
/// use hapto_webview::Asset;
///
/// // A page of one file; a built page lists its script and styles too.
/// const PAGE: &[Asset] = &[Asset {
///     path: "index.html",
///     bytes: b"<!doctype html><title>Gain</title><p>Gain</p>",
/// }];
/// assert_eq!(PAGE[0].path, "index.html");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// Its path below the page's root, with `/` separators and no leading
    /// one: `index.html`, `assets/index.js`.
    pub path: &'static str,
    /// Its bytes.
    pub bytes: &'static [u8],
}

/// What the scheme answers a request for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Served {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: &'static [u8],
}

/// Answers a request for `path` from `assets`: the file, with its content
/// type, or 404 for a path no asset has. The root is `index.html`.
pub(crate) fn serve(assets: &'static [Asset], path: &str) -> Served {
    let path = match path.trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    match assets.iter().find(|asset| asset.path == path) {
        Some(asset) => Served {
            status: 200,
            content_type: content_type(path),
            body: asset.bytes,
        },
        None => Served {
            status: 404,
            content_type: "text/plain; charset=utf-8",
            body: b"not found",
        },
    }
}

/// The content type of a file, by its extension.
fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// The URL the page loads: `dev`, a dev server's, when there is one, and
/// the page built into the plugin otherwise.
pub(crate) fn page_url(dev: Option<String>) -> String {
    dev.unwrap_or_else(|| EMBEDDED.to_owned())
}

/// Whether a document at `url` is the page. That is either the page built
/// into the plugin, which the engine loads from the origin `embedded`, or,
/// when `dev` names a dev server, a page from that server's origin. The
/// WebView goes nowhere else, and takes no message from anywhere else.
pub(crate) fn is_page(url: &str, embedded: &str, dev: Option<&str>) -> bool {
    let Some(origin) = origin(url) else {
        return false;
    };
    origin == embedded || dev.and_then(self::origin) == Some(origin)
}

/// The scheme, host and port of `url`, the part before its path; `None`
/// for a URL with no `://`.
fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    url.get(..scheme.len() + "://".len() + authority)
}

/// The dev server a debug build loads the page from: `HAPTO_PAGE_DEV_URL`,
/// when it is set and not empty.
#[cfg(debug_assertions)]
pub(crate) fn dev_url() -> Option<String> {
    std::env::var("HAPTO_PAGE_DEV_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

/// A release build never reads `HAPTO_PAGE_DEV_URL`: it always loads the page
/// built into the plugin.
#[cfg(not(debug_assertions))]
pub(crate) fn dev_url() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The origin that WebView2 loads the page built into the plugin from.
    const EMBEDDED_ORIGIN: &str = "http://hapto.localhost";

    const PAGE: &[Asset] = &[
        Asset {
            path: "index.html",
            bytes: b"<!doctype html>",
        },
        Asset {
            path: "assets/index.js",
            bytes: b"export {};",
        },
        Asset {
            path: "assets/index.css",
            bytes: b"body {}",
        },
    ];

    #[test]
    fn the_root_and_each_file_are_served_with_their_content_type() {
        for path in ["/", "", "/index.html"] {
            let served = serve(PAGE, path);
            assert_eq!(
                (served.status, served.content_type, served.body),
                (200, "text/html; charset=utf-8", &b"<!doctype html>"[..]),
                "{path:?}"
            );
        }
        let script = serve(PAGE, "/assets/index.js");
        assert_eq!(script.content_type, "text/javascript; charset=utf-8");
        assert_eq!(script.body, b"export {};");
        let styles = serve(PAGE, "/assets/index.css");
        assert_eq!(styles.content_type, "text/css; charset=utf-8");
    }

    #[test]
    fn anything_else_is_not_found() {
        for path in [
            "/missing.js",
            "/assets/",
            "/../index.html",
            "/ASSETS/INDEX.JS",
            "/index.html/",
        ] {
            let served = serve(PAGE, path);
            assert_eq!(served.status, 404, "{path:?}");
            assert_eq!(served.body, b"not found");
        }
    }

    #[test]
    fn a_file_of_an_unknown_kind_is_bytes() {
        assert_eq!(content_type("data.bin"), "application/octet-stream");
        assert_eq!(content_type("README"), "application/octet-stream");
        assert_eq!(content_type("font.woff2"), "font/woff2");
    }

    #[test]
    fn the_embedded_page_unless_a_dev_server_is_given() {
        assert_eq!(page_url(None), "hapto://localhost/index.html");
        assert_eq!(
            page_url(Some("http://localhost:5173/".into())),
            "http://localhost:5173/"
        );
    }

    #[test]
    fn only_the_page_is_the_page() {
        for url in [
            "http://hapto.localhost/index.html",
            "http://hapto.localhost/",
            "http://hapto.localhost",
            "http://hapto.localhost/assets/index.js?v=1",
        ] {
            assert!(is_page(url, EMBEDDED_ORIGIN, None), "{url}");
        }
        for url in [
            "file:///C:/Users/me/kick.wav",
            "https://example.com/",
            "http://hapto.localhost.example.com/",
            "http://hapto.localhost@example.com/",
            "http://hapto.localhost:8080/",
            "https://hapto.localhost/",
            "about:blank",
            "",
        ] {
            assert!(!is_page(url, EMBEDDED_ORIGIN, None), "{url}");
        }
    }

    #[test]
    fn a_dev_servers_page_is_the_page_only_when_it_is_given() {
        let dev = Some("http://localhost:5173/");
        assert!(!is_page("http://localhost:5173/", EMBEDDED_ORIGIN, None));
        assert!(is_page("http://localhost:5173/", EMBEDDED_ORIGIN, dev));
        assert!(is_page(
            "http://localhost:5173/src/main.tsx",
            EMBEDDED_ORIGIN,
            dev
        ));
        assert!(is_page(
            "http://localhost:5173",
            EMBEDDED_ORIGIN,
            Some("http://localhost:5173")
        ));
        assert!(!is_page("http://localhost:51730/", EMBEDDED_ORIGIN, dev));
        assert!(!is_page("http://localhost:5174/", EMBEDDED_ORIGIN, dev));
        assert!(is_page(
            "http://hapto.localhost/index.html",
            EMBEDDED_ORIGIN,
            dev
        ));
        assert!(!is_page(
            "https://example.com/",
            EMBEDDED_ORIGIN,
            Some("not a url")
        ));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn a_release_build_never_has_a_dev_server() {
        assert_eq!(dev_url(), None);
    }
}

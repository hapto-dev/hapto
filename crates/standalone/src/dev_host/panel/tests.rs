//! A panel's WebView goes to its own page alone: a dropped file or link, or a
//! page's own link, would put a page there that posts to the dev host.

use super::is_own_page;

#[test]
fn only_the_panel_s_own_page_is_navigated_to() {
    for own in ["about:blank", "data:text/html;charset=utf-8,<p>bar</p>"] {
        assert!(is_own_page(own), "{own}");
    }
    for other in [
        "file:///C:/Users/me/Downloads/page.html",
        "https://example.com/",
        "http://127.0.0.1:5173/",
        "javascript:alert(1)",
    ] {
        assert!(!is_own_page(other), "{other}");
    }
}

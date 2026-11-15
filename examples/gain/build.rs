//! Builds the gain's page in when `page/dist` holds it, by setting the
//! `seam_page` cfg `src/page.rs` reads, and warns when it does not: the
//! plugin then offers no editor. Cargo never runs Node.

use std::path::Path;

/// The files `npm run build` in `page/` writes, under fixed names.
const BUILT: [&str; 3] = [
    "page/dist/index.html",
    "page/dist/assets/index.js",
    "page/dist/assets/index.css",
];

fn main() {
    println!("cargo::rustc-check-cfg=cfg(seam_page)");
    if BUILT.iter().all(|file| Path::new(file).is_file()) {
        for file in BUILT {
            println!("cargo::rerun-if-changed={file}");
        }
        println!("cargo::rustc-cfg=seam_page");
    } else {
        // A watched file that does not exist reruns this script, and
        // rebuilds the gain, on every command. Cargo looks through a watched
        // folder for anything newer, so the page's first build is still seen.
        println!("cargo::rerun-if-changed=page");
        println!(
            "cargo::warning=the gain's page is not built, so the plugin has no editor; \
             build it with `npm ci` and `npm run build` in examples/gain/page"
        );
    }
}

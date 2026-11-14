//! The editor, served by the server itself: the built editor (`npm run build` in editor/, its
//! `dist/`) at `/`, beside the API, from the same address. One process, one port: on a machine
//! without a screen, start the server and open it from any browser that can reach it.
//!
//! Where it comes from: built into the binary (the `embed-editor` feature, as releases are
//! built: one file to install), or a folder (`--editor DIR`, or `HOLON_EDITOR`), which wins, for
//! working on the editor. Only GETs for `/` or a file name (`/assets/index-….js`) are served from
//! it, and only files inside it; every other path is the API's.

use std::path::{Component, Path, PathBuf};

use rouille::Response;

/// The built editor compiled in (editor/dist when the binary was built).
#[cfg(feature = "embed-editor")]
static EMBEDDED: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../editor/dist");

/// Where the editor's files are read from.
pub(crate) enum Source {
    Dir(PathBuf),
    /// Built into the binary.
    #[cfg_attr(not(feature = "embed-editor"), allow(dead_code))]
    Embedded,
}

impl Source {
    /// The folder given, else the built-in editor when there is one, else none.
    pub(crate) fn pick(dir: Option<PathBuf>) -> Option<Source> {
        match dir {
            Some(d) => Some(Source::Dir(d)),
            None if cfg!(feature = "embed-editor") => Some(Source::Embedded),
            None => None,
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Source::Dir(d) => format!("from {}", d.display()),
            Source::Embedded => "built in".to_string(),
        }
    }

    fn read(&self, relative: &Path) -> Option<Vec<u8>> {
        match self {
            Source::Dir(d) => std::fs::read(d.join(relative)).ok(),
            #[cfg(feature = "embed-editor")]
            Source::Embedded => EMBEDDED.get_file(relative.to_string_lossy().replace('\\', "/")).map(|f| f.contents().to_vec()),
            #[cfg(not(feature = "embed-editor"))]
            Source::Embedded => None,
        }
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" => "application/json",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" | "holon" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// What the editor's page may load and do, as the browser holds it to (its
/// `Content-Security-Policy`): nothing but what is named here.
///
/// - Scripts are the editor's own files only, never inline or from elsewhere, so markup that
///   finds its way into the page can't run; its wasm (the evaluator, `editor/wasm`) is compiled
///   from its own file (`'wasm-unsafe-eval'`, not `'unsafe-eval'`: no `eval` of text).
/// - Styles are its files and inline ones: React and CodeMirror set them on elements, and a run's
///   HTML (the HTML renderer, in a `srcdoc` frame that inherits this policy under its own
///   `INERT_POLICY`, diagnostics.ts) brings its own.
/// - Images, sound and fonts: its files, and `data:` ones: a run's value shown as an image or a
///   sound, that HTML's, and the font files small enough that the build writes them into its CSS.
/// - Requests go to the server it was served from, the marketplace this browser named (anywhere:
///   marketplace.ts) and the identity provider's token endpoint (signin.ts), so any address.
/// - No frames of it (the server's `GUARDS` say so for every other answer), no `<base>`, and no
///   form that leaves the page: every form is answered by script.
pub(crate) const PAGE_POLICY: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; \
    style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src data:; font-src 'self' data:; \
    connect-src 'self' http: https:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The file `url` names in the editor, when it is one: `/` is its `index.html`. A page is sent
/// with its policy (`PAGE_POLICY`).
pub(crate) fn serve(source: &Source, url: &str) -> Option<Response> {
    let path = url.split('?').next().unwrap_or("/");
    let last = path.rsplit('/').next().unwrap_or("");
    if path != "/" && !last.contains('.') {
        return None;
    }
    let relative = if path == "/" { PathBuf::from("index.html") } else { PathBuf::from(path.trim_start_matches('/')) };
    // Inside the editor only: no `..`, no absolute paths, no drive prefixes.
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let bytes = source.read(&relative)?;
    // The page is always read fresh; its assets carry a content hash in their names.
    let cache = if relative.starts_with("assets") { "public, max-age=31536000, immutable" } else { "no-cache" };
    let ty = content_type(&relative);
    let response = Response::from_data(ty, bytes).with_additional_header("Cache-Control", cache);
    Some(if ty.starts_with("text/html") { response.with_additional_header("Content-Security-Policy", PAGE_POLICY) } else { response })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_the_page_and_its_files_and_nothing_outside() {
        let dir = std::env::temp_dir().join(format!("holon-editor-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<html>holon</html>").unwrap();
        std::fs::write(dir.join("assets").join("app-1.js"), "export {}").unwrap();
        let source = Source::Dir(dir.clone());
        let page = serve(&source, "/").unwrap();
        assert_eq!(page.status_code, 200);
        assert!(page.headers.iter().any(|(k, v)| k == "Content-Type" && v.starts_with("text/html")));
        assert!(serve(&source, "/assets/app-1.js").is_some());
        assert!(serve(&source, "/?invite=hi_x").is_some(), "the page, with its query");
        assert!(serve(&source, "/flows").is_none(), "an API path");
        assert!(serve(&source, "/../secret.txt").is_none());
        assert!(serve(&source, "/assets/missing.js").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Built with the editor in it, the binary serves it without any folder.
    #[cfg(feature = "embed-editor")]
    #[test]
    fn the_built_in_editor_is_served() {
        let page = serve(&Source::Embedded, "/").expect("editor/dist/index.html was built in");
        let mut html = String::new();
        std::io::Read::read_to_string(&mut page.data.into_reader_and_size().0, &mut html).unwrap();
        assert!(html.contains("<div id=\"root\""), "{html}");
        assert!(serve(&Source::Embedded, "/flows").is_none(), "an API path");
    }
}

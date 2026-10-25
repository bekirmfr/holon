//! The admin page's "browse…" for a folder to bind (folders.rs). A browser can't hand a page a
//! folder's path, and the folder is on the server's machine, which needn't be the browser's, so
//! the server shows the choice:
//!
//! - `GET /admin/folders/browse?path=P`: P's folders (no P: the home folder's), its parent, the
//!   machine's roots (home, then each drive or `/`), and whether this request may open the OS's
//!   own folder dialog. The editor clicks through these when it can't.
//! - `POST /admin/folders/pick {start}`: the OS's folder dialog (Explorer's, Finder's; zenity's or
//!   kdialog's on Linux), shown by `holon pick-folder` in a process of its own, closed if nobody
//!   answers it in [`ANSWER_WITHIN`]: the folder chosen, or none. Only for a request from the
//!   machine itself (a loopback address, not one a proxy forwarded), on a machine with a desktop,
//!   and one at a time. `HOLON_NO_FOLDER_DIALOG` turns it off (the e2e tests, a shared desktop).
//!
//! Neither binds anything: the admin page binds the path chosen, checked as any other.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use rouille::{Request, Response};

use crate::http::err;
use crate::*;

/// How long the OS's dialog waits for an answer before it is closed.
const ANSWER_WITHIN: Duration = Duration::from_secs(5 * 60);

/// The most folders one listing shows.
const MAX_LISTED: usize = 1000;

/// What the dialog says it is for.
const TITLE: &str = "Choose a folder to bind";

/// A folder, as the editor shows it.
#[derive(Debug, Serialize, PartialEq)]
struct Place {
    name: String,
    path: String,
}

/// One folder's folders.
#[derive(Debug, Serialize)]
struct Listing {
    path: String,
    parent: Option<String>,
    folders: Vec<Place>,
    /// Whether there were more than [`MAX_LISTED`].
    more: bool,
    roots: Vec<Place>,
    /// Whether this request may open the OS's own dialog; if not, why.
    native: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_refused: Option<String>,
}

/// `/admin/folders/browse` and `/admin/folders/pick`, or None for any other path. Only an admin
/// gets this far (access.rs).
pub(crate) fn route(request: &Request) -> Option<Response> {
    match (request.url().as_str(), request.method()) {
        ("/admin/folders/browse", "GET") => {
            let refused = native_refused(request);
            Some(match browse(request.get_param("path").as_deref()) {
                Ok((path, parent, folders, more)) => {
                    Response::json(&Listing { path, parent, folders, more, roots: roots(), native: refused.is_none(), native_refused: refused })
                }
                Err(e) => err(400, e),
            })
        }
        ("/admin/folders/pick", "POST") => {
            if let Some(why) = native_refused(request) {
                return Some(err(409, format!("no folder dialog here: {why}")));
            }
            #[derive(Deserialize, Default)]
            struct Pick {
                #[serde(default)]
                start: Option<String>,
            }
            let start = crate::http::read_body(request).ok().and_then(|b| serde_json::from_slice::<Pick>(&b).ok()).unwrap_or_default().start;
            Some(match pick(start.as_deref().map(str::trim).filter(|s| !s.is_empty())) {
                Ok(path) => Response::json(&serde_json::json!({ "path": path })),
                Err((status, e)) => err(status, e),
            })
        }
        ("/admin/folders/browse" | "/admin/folders/pick", _) => Some(Response::empty_404()),
        _ => None,
    }
}

/// Folder `path`'s folders, by name (none given: the home folder's), with the path as the
/// machine writes it and its parent.
fn browse(path: Option<&str>) -> Result<(String, Option<String>, Vec<Place>, bool), String> {
    let at = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => crate::local::home().filter(|h| h.is_dir()).or_else(|| roots_paths().into_iter().next()).ok_or("no folder to start from")?,
    };
    if !at.is_absolute() {
        return Err(format!("{} isn't a whole path", at.display()));
    }
    let at = crate::folders::canonical(&at).map_err(|e| format!("{}: {e}", at.display()))?;
    if !at.is_dir() {
        return Err(format!("{} isn't a folder", at.display()));
    }
    let mut folders = Vec::new();
    let mut more = false;
    for entry in std::fs::read_dir(&at).map_err(|e| format!("{} can't be read: {e}", at.display()))?.flatten() {
        // A link to a folder is one.
        if !entry.path().is_dir() {
            continue;
        }
        if folders.len() == MAX_LISTED {
            more = true;
            break;
        }
        folders.push(Place { name: entry.file_name().to_string_lossy().into_owned(), path: entry.path().display().to_string() });
    }
    folders.sort_by_key(|p| p.name.to_lowercase());
    Ok((at.display().to_string(), at.parent().map(|p| p.display().to_string()), folders, more))
}

/// Where a machine's folders start: the home folder, then each drive (Windows) or `/`.
fn roots() -> Vec<Place> {
    let home = crate::local::home().filter(|h| h.is_dir()).map(|h| Place { name: "home".to_string(), path: h.display().to_string() });
    home.into_iter().chain(roots_paths().into_iter().map(|p| Place { name: p.display().to_string(), path: p.display().to_string() })).collect()
}

#[cfg(windows)]
fn roots_paths() -> Vec<PathBuf> {
    // A bitmask of the drives there are: looking at each could wait on a network drive.
    let drives = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8).filter(|i| drives & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
}

#[cfg(not(windows))]
fn roots_paths() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

/// Why `request` may not open the OS's folder dialog, if it may not: it must come from the
/// machine itself, which must have a desktop to show it on.
fn native_refused(request: &Request) -> Option<String> {
    let forwarded = request.header("X-Forwarded-For").is_some() || request.header("Forwarded").is_some();
    if !request.remote_addr().ip().is_loopback() || forwarded {
        return Some("the editor isn't on the server's machine".to_string());
    }
    desktop_refused()
}

/// Why the server's machine can't show a folder dialog, if it can't.
fn desktop_refused() -> Option<String> {
    if std::env::var_os("HOLON_NO_FOLDER_DIALOG").is_some() {
        return Some("HOLON_NO_FOLDER_DIALOG is set".to_string());
    }
    #[cfg(windows)]
    {
        // Session 0 is the services': it has no desktop anyone sees.
        let mut session = 0u32;
        let ok = unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(windows_sys::Win32::System::Threading::GetCurrentProcessId(), &mut session) };
        if ok != 0 && session == 0 {
            return Some("the server runs as a service, with no desktop".to_string());
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        None
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Some("the server's machine has no desktop".to_string());
        }
        linux_dialog().is_none().then(|| "neither zenity nor kdialog is installed".to_string())
    }
}

/// The OS's folder dialog, from `holon pick-folder` in a process of its own: the folder chosen,
/// or none if it was cancelled.
fn pick(start: Option<&str>) -> Result<Option<String>, (u16, String)> {
    static OPEN: Mutex<()> = Mutex::new(());
    let _one = match OPEN.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Err((409, "a folder dialog is open already: answer it first".to_string())),
    };
    let me = std::env::current_exe().map_err(|e| (500, format!("can't find this program to show the dialog: {e}")))?;
    let mut cmd = Command::new(me);
    cmd.arg("pick-folder").args(start).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // A console program: without this, a server with no console of its own would open one beside the dialog.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000 /* CREATE_NO_WINDOW */);
    let mut child = cmd.spawn().map_err(|e| (500, format!("the folder dialog didn't start: {e}")))?;
    let asked = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if asked.elapsed() >= ANSWER_WITHIN => {
                let _ = child.kill();
                let _ = child.wait();
                return Err((408, format!("nobody chose a folder within {} minutes: the dialog was closed", ANSWER_WITHIN.as_secs() / 60)));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err((500, e.to_string())),
        }
    }
    let out = child.wait_with_output().map_err(|e| (500, e.to_string()))?;
    match out.status.code() {
        Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|p| !p.is_empty())),
        Some(1) => Ok(None),
        _ => Err((500, format!("the folder dialog failed: {}", String::from_utf8_lossy(&out.stderr).trim()))),
    }
}

/// `holon pick-folder [START]`, run by the server for the admin page: the OS's folder dialog,
/// starting in START. The folder chosen on stdout (exit 0), nothing if cancelled (1), why it
/// couldn't be shown on stderr (2).
pub(crate) fn cli(args: &[String]) -> i32 {
    match show(args.first().map(Path::new)) {
        Ok(Some(path)) => {
            println!("{}", path.display());
            0
        }
        Ok(None) => 1,
        Err(e) => {
            eprintln!("holon pick-folder: {e}");
            2
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn dialog(start: Option<&Path>) -> rfd::FileDialog {
    let d = rfd::FileDialog::new().set_title(TITLE);
    match start.filter(|s| s.is_dir()) {
        Some(s) => d.set_directory(s),
        None => d,
    }
}

#[cfg(windows)]
fn show(start: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let owner = topmost::Owner::new();
    let mut d = dialog(start);
    if let Some(o) = &owner {
        d = d.set_parent(o);
    }
    Ok(d.pick_folder())
}

#[cfg(target_os = "macos")]
fn show(start: Option<&Path>) -> Result<Option<PathBuf>, String> {
    Ok(dialog(start).pick_folder())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show(start: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let tool = linux_dialog().ok_or("neither zenity nor kdialog is installed")?;
    let mut cmd = Command::new(tool);
    if tool == "zenity" {
        cmd.args(["--file-selection", "--directory", "--title", TITLE]);
        if let Some(s) = start {
            cmd.arg(format!("--filename={}/", s.display()));
        }
    } else {
        cmd.args(["--title", TITLE, "--getexistingdirectory"]);
        cmd.arg(start.unwrap_or(Path::new(".")));
    }
    let out = cmd.output().map_err(|e| format!("{tool} didn't run: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|p| !p.is_empty()).map(PathBuf::from)),
        Some(1) => Ok(None),
        _ => Err(format!("{tool} failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// The folder dialog Linux has: zenity (GNOME's) or kdialog (KDE's), whichever is on the PATH.
#[cfg(not(any(windows, target_os = "macos")))]
fn linux_dialog() -> Option<&'static str> {
    let paths = std::env::var_os("PATH")?;
    ["zenity", "kdialog"].into_iter().find(|tool| std::env::split_paths(&paths).any(|d| d.join(tool).is_file()))
}

/// An invisible window above all others that owns the dialog, so the dialog opens in front of the
/// browser that asked for it rather than behind: a process that didn't have the user's last input
/// can't take the foreground, but what a topmost window owns is topmost too.
#[cfg(windows)]
mod topmost {
    use raw_window_handle::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP};

    pub(super) struct Owner(HWND);

    impl Owner {
        pub(super) fn new() -> Option<Owner> {
            // A built-in class: nothing to register.
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let null = std::ptr::null_mut();
            let hwnd = unsafe { CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 0, 0, null, null, null, std::ptr::null()) };
            (!hwnd.is_null()).then_some(Owner(hwnd))
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    impl HasWindowHandle for Owner {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let hwnd = std::num::NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
            // It lives as long as the Owner the handle borrows.
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
        }
    }

    impl HasDisplayHandle for Owner {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_has_a_folders_folders_by_name_and_its_parent() {
        let dir = std::env::temp_dir().join(format!("holon-browse-{}-{}", std::process::id(), rand::random::<u32>()));
        for d in ["b", "A", "c/inner"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("file.txt"), "not a folder").unwrap();
        let (path, parent, folders, more) = browse(Some(dir.to_str().unwrap())).unwrap();
        let real = crate::folders::canonical(&dir).unwrap();
        assert_eq!((path.as_str(), parent, more), (real.to_str().unwrap(), real.parent().map(|p| p.display().to_string()), false));
        assert_eq!(folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["A", "b", "c"], "folders only, by name");
        assert_eq!(folders[2].path, real.join("c").display().to_string());
        assert!(browse(Some("relative/path")).unwrap_err().contains("isn't a whole path"));
        assert!(browse(Some(dir.join("file.txt").to_str().unwrap())).unwrap_err().contains("isn't a folder"));
        assert!(browse(Some(dir.join("none").to_str().unwrap())).is_err());
        assert!(browse(None).is_ok(), "the home folder's");
        assert!(!roots().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

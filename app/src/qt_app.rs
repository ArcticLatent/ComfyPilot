//! Application-level Qt setup (see `cpp/arctic_qt.h`): the widgets-capable
//! application object, bundled fonts, window icon, desktop app id,
//! Qt/QML messages copied into the app's log file, and the disk cache for
//! QML's network access.

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qqmlapplicationengine.h");
        type QQmlApplicationEngine = cxx_qt_lib::QQmlApplicationEngine;
        include!("arctic_qt.h");

        #[namespace = "arctic"]
        fn create_application(
            args: &QStringList,
            name: &QString,
            organization: &QString,
            version: &QString,
        );
        #[namespace = "arctic"]
        fn exec_application() -> i32;
        #[namespace = "arctic"]
        fn set_window_icon(path: &QString);
        #[namespace = "arctic"]
        fn set_desktop_file_name(name: &QString);
        #[namespace = "arctic"]
        fn add_application_font(path: &QString) -> bool;
        #[namespace = "arctic"]
        fn set_application_font_family(family: &QString);
        #[namespace = "arctic"]
        fn install_message_log(path: &QString);
        #[namespace = "arctic"]
        fn enable_network_disk_cache(engine: Pin<&mut QQmlApplicationEngine>);
        #[namespace = "arctic"]
        fn claim_single_instance(name: &QString) -> bool;
        #[namespace = "arctic"]
        fn save_main_window_image(path: &QString) -> bool;
        #[namespace = "arctic"]
        fn engine_loaded(engine: Pin<&mut QQmlApplicationEngine>) -> bool;
    }
}

use cxx_qt_lib::{QString, QStringList};

/// The bundled fonts (in the QML module's resources).
const FONTS: &[&str] = &[
    "Geist-Regular",
    "Geist-Medium",
    "Geist-SemiBold",
    "Geist-Bold",
    "JetBrainsMono-Regular",
    "JetBrainsMono-Medium",
];
const RESOURCES: &str = ":/qt/qml/ArcticHelper/qml";

/// Creates the application object and sets it up: fonts, icon, app id, and
/// Qt messages in the log file.
pub fn create_application() {
    let mut args = QStringList::default();
    for arg in std::env::args() {
        args.append(QString::from(&arg));
    }
    ffi::create_application(
        &args,
        &QString::from("ComfyPilot"),
        &QString::from("Arctic Latent"),
        &QString::from(env!("CARGO_PKG_VERSION")),
    );
    for font in FONTS {
        if !ffi::add_application_font(&QString::from(&format!("{RESOURCES}/fonts/{font}.ttf"))) {
            log::warn!("Couldn't load the bundled font {font}");
        }
    }
    ffi::set_application_font_family(&QString::from("Geist"));
    ffi::set_window_icon(&QString::from(&format!("{RESOURCES}/assets/icon.svg")));
    // Only with the desktop entry installed (a package): the desktop portal
    // refuses names it can't find, as under `cargo run`.
    if desktop_entry_installed() {
        ffi::set_desktop_file_name(&QString::from(arctic_downloader::app::APP_ID));
    }
    if let Some(path) = arctic_downloader::applog::log_file_path() {
        ffi::install_message_log(&QString::from(path.to_string_lossy().as_ref()));
    }
}

/// Gives QML's network access (Civitai previews) a disk cache. Call before
/// loading QML.
pub fn enable_network_disk_cache(engine: std::pin::Pin<&mut cxx_qt_lib::QQmlApplicationEngine>) {
    ffi::enable_network_disk_cache(engine);
}

/// Saves an image of the main window, popups included (`--screenshot=`).
pub fn save_main_window_image(path: &cxx_qt_lib::QString) -> bool {
    ffi::save_main_window_image(path)
}

/// Whether the engine loaded its main QML file.
pub fn engine_loaded(engine: std::pin::Pin<&mut cxx_qt_lib::QQmlApplicationEngine>) -> bool {
    ffi::engine_loaded(engine)
}

/// Whether this is the only running instance (for this user). A second one
/// asks the first to show its window and should exit.
pub fn claim_single_instance() -> bool {
    ffi::claim_single_instance(&QString::from(&single_instance_name()))
}

/// The single-instance socket: in the runtime folder on Linux (shared by a
/// Flatpak's instances, private to the user), else a per-user name (in the
/// temp folder on Linux, a pipe name on Windows).
fn single_instance_name() -> String {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let name = format!(
        "{}-{}",
        arctic_downloader::app::APP_ID,
        user.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
    );
    if cfg!(target_os = "linux") {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
        let flatpak = std::env::var("FLATPAK_ID").ok();
        let dir = match (runtime, flatpak) {
            (Some(runtime), Some(id)) => Some(runtime.join("app").join(id)),
            (Some(runtime), None) => Some(runtime),
            (None, _) => None,
        };
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            let path = dir
                .join(format!("{name}.sock"))
                .to_string_lossy()
                .to_string();
            // A Unix socket path has room for about 107 bytes.
            if path.len() < 100 {
                return path;
            }
        }
    }
    name
}

/// Runs the event loop until the app quits; returns the exit code.
pub fn exec() -> i32 {
    ffi::exec_application()
}

/// Whether `<APP_ID>.desktop` is in an XDG `applications` folder (Linux).
fn desktop_entry_installed() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let file = format!("applications/{}.desktop", arctic_downloader::app::APP_ID);
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        });
    let dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(
            dirs.split(':')
                .filter(|d| !d.is_empty())
                .map(std::path::PathBuf::from),
        )
        .any(|dir| dir.join(&file).is_file())
}

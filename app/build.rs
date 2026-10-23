use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

/// QML files of the `ArcticHelper` module, relative to this crate.
const QML_FILES: &[&str] = &[
    "qml/Main.qml",
    "qml/Icon.qml",
    "qml/ControlSurface.qml",
    "qml/FlatButton.qml",
    "qml/PanelButton.qml",
    "qml/PanelHeader.qml",
    "qml/PageHeader.qml",
    "qml/AppTextField.qml",
    "qml/AppCheckBox.qml",
    "qml/AppSwitch.qml",
    "qml/ComboField.qml",
    "qml/Chip.qml",
    "qml/CountBadge.qml",
    "qml/StatusDot.qml",
    "qml/LogoMark.qml",
    "qml/LeftRail.qml",
    "qml/DockBar.qml",
    "qml/Sidebar.qml",
    "qml/ActivityPanel.qml",
    "qml/DownloadsPanel.qml",
    "qml/AppDialog.qml",
    "qml/DialogButton.qml",
    "qml/AppButtonBox.qml",
    "qml/ConfirmDialog.qml",
    "qml/SectionCard.qml",
    "qml/OptionRow.qml",
    "qml/OptionTile.qml",
    "qml/OptionGrid.qml",
    "qml/FolderField.qml",
    "qml/InfoTile.qml",
    "qml/LaunchOptions.qml",
    "qml/ComfyPage.qml",
    "qml/ComfyManageView.qml",
    "qml/ComfyInstallView.qml",
    "qml/ModelsPage.qml",
    "qml/CivitaiBrowser.qml",
    "qml/WorkflowsPage.qml",
    "qml/AppTray.qml",
    "qml/StartupOverlay.qml",
    "qml/ConsolePanel.qml",
    "qml/StepList.qml",
    "qml/PopupSurface.qml",
    "qml/PopupScrim.qml",
];
const SINGLETONS: &[&str] = &["qml/Api.qml", "qml/Theme.qml", "qml/Downloads.qml"];
/// Also JavaScript libraries the QML imports by relative path.
const ASSETS: &[&str] = &[
    "qml/ModelLogic.js",
    "qml/ConsoleFormat.js",
    "qml/fonts/Geist-Regular.ttf",
    "qml/fonts/Geist-Medium.ttf",
    "qml/fonts/Geist-SemiBold.ttf",
    "qml/fonts/Geist-Bold.ttf",
    "qml/fonts/JetBrainsMono-Regular.ttf",
    "qml/fonts/JetBrainsMono-Medium.ttf",
    "qml/assets/icon.svg",
    "qml/assets/logo.svg",
    "qml/assets/logo-light.svg",
    "qml/assets/icon-running.svg",
    "qml/assets/no-preview.jpg",
    "qml/assets/no-preview-light.jpg",
];

fn main() {
    println!("cargo::rerun-if-changed=cpp");
    let mut qml: Vec<QmlFile> = QML_FILES.iter().map(|path| QmlFile::from(*path)).collect();
    qml.extend(
        SINGLETONS
            .iter()
            .map(|path| QmlFile::from(*path).singleton(true)),
    );
    // Assets are served under the module's prefix:
    // qrc:/qt/qml/ArcticHelper/qml/assets/<name>.
    CxxQtBuilder::new_qml_module(QmlModule::new("ArcticHelper").qml_files(qml))
        .qrc_resources(ASSETS.iter().copied())
        // QApplication (the tray needs widgets on KDE) and the helpers in cpp/.
        .qt_module("Widgets")
        // QQuickWindow::grabWindow (cpp/arctic_qt.h).
        .qt_module("Quick")
        // The disk cache for QML's network access (cpp/arctic_qt.h).
        .qt_module("Network")
        .include_dir("cpp")
        .files(["src/bridge.rs", "src/qt_app.rs"])
        .build();

    // ld.bfd with --as-needed (Fedora's and Arch's Rust; rustup's uses lld)
    // keeps a shared library only if something linked before it uses it, and
    // cxx-qt lists Qt before its static library that calls Qt. List them
    // again after it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for lib in [
            "Qt6Widgets",
            "Qt6Quick",
            "Qt6QuickControls2",
            "Qt6Qml",
            "Qt6Network",
            "Qt6Gui",
            "Qt6Core",
        ] {
            println!("cargo::rustc-link-lib=dylib={lib}");
        }
    }

    // Explorer, the Start menu and the installer show the .exe's own icon.
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("icons/icon.ico")
        .set("ProductName", "ComfyPilot")
        .set("FileDescription", "ComfyPilot")
        .set("CompanyName", "Arctic Latent")
        .compile()
        .expect("embed the Windows icon and version info");
}

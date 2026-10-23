// Application-level Qt setup the QML side can't do: the application object,
// bundled fonts, window icon, desktop app id, Qt/QML messages in the app's
// log file, a disk cache for what QML loads over the network, and the
// single-instance guard. Adapted from Harlo's harlo_platform.h / harlo_fonts.h.
#pragma once

#include <QtCore/QByteArray>
#include <QtCore/QDateTime>
#include <QtCore/QFile>
#include <QtCore/QLoggingCategory>
#include <QtCore/QMutex>
#include <QtCore/QStandardPaths>
#include <QtCore/QString>
#include <QtCore/QStringList>
#include <QtCore/QTextStream>
#include <QtGui/QFont>
#include <QtGui/QFontDatabase>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtGui/QWindow>
#include <QtNetwork/QLocalServer>
#include <QtNetwork/QLocalSocket>
#include <QtNetwork/QNetworkAccessManager>
#include <QtNetwork/QNetworkDiskCache>
#include <QtNetwork/QNetworkReply>
#include <QtNetwork/QNetworkRequest>
#include <QtQml/QQmlApplicationEngine>
#include <QtQuick/QQuickWindow>
#include <QtQml/QQmlNetworkAccessManagerFactory>
#include <QtWidgets/QApplication>
#include <QtWidgets/QMenu>
#include <QtCore/QEvent>

#include <vector>

namespace arctic {

// On Wayland a popup needs a parent window that has had input. A menu
// without one (the tray menu, which Qt can pop up on its own) fails with
// "Failed to create grabbing popup" and never appears; the desktop shows the
// tray menu itself over D-Bus. So such a menu is kept off the screen (the
// show event comes before the window is mapped) and closed straight away.
class WaylandOrphanMenuGuard : public QObject
{
public:
    using QObject::QObject;

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        if (event->type() == QEvent::Show) {
            auto *menu = qobject_cast<QMenu *>(watched);
            if (menu && !menu->parentWidget() && !(menu->windowHandle() && menu->windowHandle()->transientParent())) {
                menu->setAttribute(Qt::WA_DontShowOnScreen, true);
                QMetaObject::invokeMethod(menu, &QWidget::close, Qt::QueuedConnection);
            }
        }
        return QObject::eventFilter(watched, event);
    }
};

// Creates the application object: a QApplication (widgets-capable), so the
// desktop's tray integration (KDE's needs widgets) and native dialogs work.
// `args` become argv, kept alive for the application's lifetime.
inline void create_application(const QStringList &args, const QString &name, const QString &organization,
                               const QString &version)
{
    static std::vector<QByteArray> storage;
    static std::vector<char *> argv;
    static int argc = 0;
    for (const QString &a : args)
        storage.push_back(a.toLocal8Bit());
    for (QByteArray &s : storage)
        argv.push_back(s.data());
    argv.push_back(nullptr);
    argc = int(storage.size());
    new QApplication(argc, argv.data());
    // The window may close to the tray; QML quits explicitly (Qt.quit()).
    QGuiApplication::setQuitOnLastWindowClosed(false);
    if (QGuiApplication::platformName() == QLatin1String("wayland"))
        qApp->installEventFilter(new WaylandOrphanMenuGuard(qApp));
    QCoreApplication::setApplicationName(name);
    QCoreApplication::setOrganizationName(organization);
    QCoreApplication::setApplicationVersion(version);
}

// Runs the event loop until the app quits; returns the exit code.
inline int exec_application()
{
    return QApplication::exec();
}

// The icon of every window (a resource or file path).
inline void set_window_icon(const QString &path)
{
    QGuiApplication::setWindowIcon(QIcon(path));
}

// The desktop entry name (Wayland app id), so the desktop shows the right icon.
inline void set_desktop_file_name(const QString &name)
{
    QGuiApplication::setDesktopFileName(name);
}

// Adds a font file (a resource) to the application's fonts; false if it
// couldn't be read.
inline bool add_application_font(const QString &path)
{
    return QFontDatabase::addApplicationFont(path) >= 0;
}

// Sets the application font family, keeping its size.
inline void set_application_font_family(const QString &family)
{
    QFont font = QGuiApplication::font();
    font.setFamily(family);
    QGuiApplication::setFont(font);
}

// Copies Qt and QML messages (warnings, binding errors...) into the log file
// at `path`, keeping the normal output (stderr or the journal).
inline void install_message_log(const QString &path)
{
    static QFile file;
    static QMutex mutex;
    static QtMessageHandler previous = nullptr;
    file.setFileName(path);
    if (!file.open(QIODevice::Append | QIODevice::Text))
        return;
    previous = qInstallMessageHandler([](QtMsgType type, const QMessageLogContext &ctx, const QString &msg) {
        const char *level = type == QtDebugMsg ? "DEBUG"
                          : type == QtInfoMsg ? "INFO"
                          : type == QtWarningMsg ? "WARN"
                          : type == QtCriticalMsg ? "ERROR"
                                                  : "FATAL";
        {
            QMutexLocker lock(&mutex);
            QTextStream out(&file);
            out << "[" << QDateTime::currentDateTimeUtc().toString(Qt::ISODate) << " " << level << " qt";
            if (ctx.category && qstrcmp(ctx.category, "default") != 0)
                out << " " << ctx.category;
            out << "] " << msg;
            if (ctx.file)
                out << " (" << ctx.file << ":" << ctx.line << ")";
            out << "\n";
            out.flush();
        }
        if (previous)
            previous(type, ctx, msg);
    });
}

// `arctic.netcache` (off by default): where each network reply came from.
inline const QLoggingCategory &netcache_log()
{
    static const QLoggingCategory category("arctic.netcache", QtWarningMsg);
    return category;
}

// QML's network access (images) with a disk cache. Civitai's preview URLs
// are content-addressed, but the CDN serves them already past their
// one-hour max-age, so for those hosts the cached copy is used whatever its
// age; other requests follow the usual HTTP caching rules.
class CachingNetworkAccessManager : public QNetworkAccessManager
{
public:
    using QNetworkAccessManager::QNetworkAccessManager;

protected:
    QNetworkReply *createRequest(Operation op, const QNetworkRequest &original, QIODevice *data) override
    {
        QNetworkRequest request(original);
        const QString host = request.url().host();
        if (op == GetOperation && (host == QLatin1String("image.civitai.com") || host.endsWith(QLatin1String(".civitai.com"))))
            request.setAttribute(QNetworkRequest::CacheLoadControlAttribute, QNetworkRequest::PreferCache);
        QNetworkReply *reply = QNetworkAccessManager::createRequest(op, request, data);
        if (netcache_log().isDebugEnabled()) {
            connect(reply, &QNetworkReply::finished, reply, [reply] {
                qCDebug(netcache_log) << (reply->attribute(QNetworkRequest::SourceIsFromCacheAttribute).toBool() ? "cache" : "network")
                                      << reply->url().toString();
            });
        }
        return reply;
    }
};

class CachingNetworkFactory : public QQmlNetworkAccessManagerFactory
{
public:
    explicit CachingNetworkFactory(const QString &directory)
        : m_directory(directory)
    {
    }

    // Called once per thread that loads (the image reader has its own); the
    // caches share the folder, each one writing whole files.
    QNetworkAccessManager *create(QObject *parent) override
    {
        auto *manager = new CachingNetworkAccessManager(parent);
        auto *cache = new QNetworkDiskCache(manager);
        cache->setCacheDirectory(m_directory);
        cache->setMaximumCacheSize(qint64(300) * 1024 * 1024);
        manager->setCache(cache);
        return manager;
    }

private:
    QString m_directory;
};

// Gives the engine's network access a disk cache in the app's cache folder
// (`<cache>/network`). Call before loading QML.
inline void enable_network_disk_cache(QQmlApplicationEngine &engine)
{
    static CachingNetworkFactory factory(QStandardPaths::writableLocation(QStandardPaths::CacheLocation) + QStringLiteral("/network"));
    engine.setNetworkAccessManagerFactory(&factory);
}

// Saves an image of the main window, popups and dialogs included (the
// development `--screenshot=` option). False when there's no such window or
// the file can't be written.
inline bool save_main_window_image(const QString &path)
{
    for (QWindow *window : QGuiApplication::topLevelWindows()) {
        if (window->objectName() == QLatin1String("mainWindow")) {
            if (auto *quick = qobject_cast<QQuickWindow *>(window))
                return quick->grabWindow().save(path);
        }
    }
    return false;
}

// Whether the engine has a root object: false when Main.qml failed to load.
inline bool engine_loaded(QQmlApplicationEngine &engine)
{
    return !engine.rootObjects().isEmpty();
}

// Shows and raises the main window (the QML window named "mainWindow").
inline void show_main_window()
{
    for (QWindow *window : QGuiApplication::topLevelWindows()) {
        if (window->objectName() == QLatin1String("mainWindow")) {
            window->show();
            if (window->windowStates() & Qt::WindowMinimized)
                window->setWindowStates(window->windowStates() & ~Qt::WindowMinimized);
            window->raise();
            window->requestActivate();
            return;
        }
    }
}

// One instance per user: true when this is the first, which then listens on
// `name` (a local socket path, or a pipe name on Windows) and shows its window
// when another one starts. A later instance tells the first to show itself
// and gets false. If the server can't listen, the app runs anyway.
inline bool claim_single_instance(const QString &name)
{
    {
        QLocalSocket probe;
        probe.connectToServer(name);
        if (probe.waitForConnected(500)) {
            probe.write("show\n");
            probe.waitForBytesWritten(500);
            probe.disconnectFromServer();
            return false;
        }
    }
    // Nobody answered: a socket file left by a crash is stale.
    QLocalServer::removeServer(name);
    auto *server = new QLocalServer(qApp);
    server->setSocketOptions(QLocalServer::UserAccessOption);
    if (!server->listen(name)) {
        qWarning("Single-instance guard unavailable: %s", qPrintable(server->errorString()));
        return true;
    }
    QObject::connect(server, &QLocalServer::newConnection, server, [server] {
        while (QLocalSocket *connection = server->nextPendingConnection())
            QObject::connect(connection, &QLocalSocket::disconnected, connection, &QObject::deleteLater);
        show_main_window();
    });
    return true;
}

} // namespace arctic

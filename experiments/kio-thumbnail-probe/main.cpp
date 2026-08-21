#include <KFileItem>
#include <KIO/PreviewJob>
#include <KPluginMetaData>

#include <QApplication>
#include <QDir>
#include <QImage>
#include <QMimeDatabase>
#include <QPixmap>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTimer>
#include <QUrl>

#include <chrono>
#include <cstdio>
#include <memory>

using Clock = std::chrono::steady_clock;

static void print_plugins()
{
    QJsonArray plugins;
    for (const auto &plugin : KIO::PreviewJob::availableThumbnailerPlugins()) {
        QJsonObject object;
        object.insert(QStringLiteral("name"), plugin.name());
        object.insert(QStringLiteral("file"), plugin.fileName());
        object.insert(QStringLiteral("description"), plugin.description());
        object.insert(QStringLiteral("mime_types"), QJsonArray::fromStringList(plugin.mimeTypes()));
        plugins.append(object);
    }
    QJsonObject result;
    result.insert(QStringLiteral("supported_mime_types"), QJsonArray::fromStringList(KIO::PreviewJob::supportedMimeTypes()));
    result.insert(QStringLiteral("available_plugins"), QJsonArray::fromStringList(KIO::PreviewJob::availablePlugins()));
    result.insert(QStringLiteral("default_plugins"), QJsonArray::fromStringList(KIO::PreviewJob::defaultPlugins()));
    result.insert(QStringLiteral("plugins"), plugins);
    std::puts(QJsonDocument(result).toJson(QJsonDocument::Compact).constData());
}

static int preview(const QString &file, int size, const QString &output_path)
{
    QApplication *application = qobject_cast<QApplication *>(QCoreApplication::instance());
    const auto started = Clock::now();
    const QString output = output_path.isEmpty()
        ? QDir::temp().filePath(QStringLiteral("kio-thumbnail-probe-%1.png").arg(QCoreApplication::applicationPid()))
        : output_path;
    const QUrl url = QUrl::fromLocalFile(file);
    const QString mime = QMimeDatabase().mimeTypeForFile(file).name();
    const QStringList plugins = KIO::PreviewJob::availablePlugins();
    KIO::PreviewJob *job = KIO::filePreview(KFileItemList{KFileItem(url, mime)}, QSize(size, size), &plugins);
    job->setScaleType(KIO::PreviewJob::ScaledAndCached);
    const auto reported = std::make_shared<bool>(false);
    const auto report_success = [started, application, reported, output](const QImage &image) {
        if (*reported) {
            return;
        }
        *reported = true;
        if (!image.save(output, "PNG")) {
            std::fputs("{\"result\":\"error\",\"error\":\"output_write_failed\"}\n", stdout);
            QCoreApplication::exit(4);
            return;
        }
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - started).count();
        QJsonObject result;
        result.insert(QStringLiteral("result"), QStringLiteral("thumbnail"));
        result.insert(QStringLiteral("width"), image.width());
        result.insert(QStringLiteral("height"), image.height());
        result.insert(QStringLiteral("output_path"), output);
        result.insert(QStringLiteral("elapsed_ms"), static_cast<qint64>(elapsed));
        result.insert(QStringLiteral("cache_path"), QDir::homePath() + QStringLiteral("/.cache/thumbnails"));
        std::puts(QJsonDocument(result).toJson(QJsonDocument::Compact).constData());
        QCoreApplication::quit();
    };
    QObject::connect(job, &KIO::PreviewJob::generated, application, [report_success](const KFileItem &, const QImage &image) {
        report_success(image);
    });
    QObject::connect(job, &KIO::PreviewJob::gotPreview, application, [report_success](const KFileItem &, const QPixmap &preview) {
        report_success(preview.toImage());
    });
    QObject::connect(job, &KIO::PreviewJob::failed, application, [started](const KFileItem &) {
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - started).count();
        QJsonObject result;
        result.insert(QStringLiteral("result"), QStringLiteral("unavailable_or_error"));
        result.insert(QStringLiteral("elapsed_ms"), static_cast<qint64>(elapsed));
        std::puts(QJsonDocument(result).toJson(QJsonDocument::Compact).constData());
        QCoreApplication::exit(2);
    });
    QTimer::singleShot(15000, application, [started, job] {
        const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - started).count();
        QJsonObject result;
        result.insert(QStringLiteral("result"), QStringLiteral("timeout"));
        result.insert(QStringLiteral("elapsed_ms"), static_cast<qint64>(elapsed));
        std::puts(QJsonDocument(result).toJson(QJsonDocument::Compact).constData());
        job->kill(KJob::EmitResult);
        QCoreApplication::exit(3);
    });
    return application->exec();
}

int main(int argc, char **argv)
{
    QApplication application(argc, argv);
    if (argc >= 2 && QString::fromLocal8Bit(argv[1]) == QStringLiteral("plugins")) {
        print_plugins();
        return 0;
    }
    if (argc < 3 || QString::fromLocal8Bit(argv[1]) != QStringLiteral("preview")) {
        std::fprintf(stderr, "usage: kio-thumbnail-probe plugins | preview FILE [SIZE]\n");
        return 64;
    }
    return preview(QString::fromLocal8Bit(argv[2]), argc >= 4 ? QByteArray(argv[3]).toInt() : 256,
                   argc >= 5 ? QString::fromLocal8Bit(argv[4]) : QString());
}

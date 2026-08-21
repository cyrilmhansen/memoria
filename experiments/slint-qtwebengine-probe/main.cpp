#include <QApplication>
#include <QFile>
#include <QMessageBox>
#include <QTimer>
#include <QUrl>
#include <QWebEnginePage>
#include <QWebEngineProfile>
#include <QWebEngineSettings>
#include <QWebEngineUrlRequestInfo>
#include <QWebEngineUrlRequestInterceptor>
#include <QWebEngineView>
#include <QVBoxLayout>
#include <QWidget>

class LocalOnlyInterceptor final : public QWebEngineUrlRequestInterceptor {
public:
    void interceptRequest(QWebEngineUrlRequestInfo &info) override {
        if (info.requestUrl().scheme() != "file" && info.requestUrl().scheme() != "data") {
            info.block(true);
        }
    }
};

class ProbePage final : public QWebEnginePage {
public:
    using QWebEnginePage::QWebEnginePage;

protected:
    bool acceptNavigationRequest(const QUrl &url, NavigationType, bool) override {
        const bool allowed = url.scheme() == "file" || url.scheme() == "data";
        qInfo().noquote() << "navigation_allowed=" << allowed << "scheme=" << url.scheme();
        return allowed;
    }
};

static QString html() {
    return QStringLiteral(R"HTML(<!doctype html>
<meta charset="utf-8"><title>Qt WebEngine local probe</title>
<style>body{font:16px sans-serif;margin:24px}table{border-collapse:collapse}td{border:1px solid #999;padding:6px}.box{color:#174c9c;background:#e7efff;padding:10px}</style>
<h1>Qt WebEngine local probe</h1><p><b>bold</b>, local-only content and a blocked external link.</p>
<p class="box">This document must stay inside the Qt WebEngine view.</p>
<table><tr><td>resize</td><td>focus</td></tr><tr><td>scroll</td><td>link policy</td></tr></table>
<p><a href="https://example.invalid/">external link (must be blocked)</a></p>
<input aria-label="local test input" placeholder="focus test"><div style="height:2400px"></div>
)HTML");
}

int main(int argc, char **argv) {
    QApplication app(argc, argv);
    QWebEngineProfile profile(QStringLiteral("slint-qtwebengine-probe"), &app);
    profile.setUrlRequestInterceptor(new LocalOnlyInterceptor);
    profile.settings()->setAttribute(QWebEngineSettings::JavascriptEnabled, false);
    profile.settings()->setAttribute(QWebEngineSettings::LocalContentCanAccessRemoteUrls, false);
    profile.settings()->setAttribute(QWebEngineSettings::LocalContentCanAccessFileUrls, true);
    profile.settings()->setAttribute(QWebEngineSettings::AutoLoadImages, true);

    QWidget window;
    window.setWindowTitle(QStringLiteral("Slint + Qt WebEngine feasibility probe"));
    window.resize(900, 650);
    auto *layout = new QVBoxLayout(&window);
    auto *view = new QWebEngineView(&window);
    view->setPage(new ProbePage(&profile, view));
    view->setFocusPolicy(Qt::StrongFocus);
    view->setHtml(html(), QUrl(QStringLiteral("file:///slint-qtwebengine-probe/")));
    QObject::connect(view, &QWebEngineView::loadFinished, [](bool ok) {
        qInfo() << "local_html_loaded=" << ok;
    });
    layout->addWidget(view);
    window.show();
    qInfo() << "qt_platform=" << qEnvironmentVariable("QT_QPA_PLATFORM", "auto")
            << "javascript_enabled=" << false
            << "network_policy=only file/data navigation; non-local requests blocked";

    if (app.arguments().contains(QStringLiteral("--smoke"))) {
        QTimer::singleShot(2500, &app, &QCoreApplication::quit);
    }
    return app.exec();
}

#include <QApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QListWidget>
#include <QMainWindow>
#include <QPushButton>
#include <QTextBrowser>
#include <QTextDocument>
#include <QTimer>
#include <QUrl>
#include <QVBoxLayout>
#include <QLabel>
#include <algorithm>

class LocalTextBrowser final : public QTextBrowser {
public:
    explicit LocalTextBrowser(const QString &root, QWidget *parent = nullptr)
        : QTextBrowser(parent), root_(QFileInfo(root).canonicalFilePath()) {
        setOpenLinks(false);
        setOpenExternalLinks(false);
        connect(this, &QTextBrowser::anchorClicked, this, [this](const QUrl &url) {
            qInfo().noquote() << "link_activated scheme=" << url.scheme()
                              << "external=" << (url.scheme() == "http" || url.scheme() == "https");
        });
    }

protected:
    QVariant loadResource(int type, const QUrl &url) override {
        if (url.scheme() == "http" || url.scheme() == "https" ||
            (!url.scheme().isEmpty() && url.scheme() != "file" && url.scheme() != "data")) {
            qInfo().noquote() << "resource_blocked scheme=" << url.scheme();
            return {};
        }
        if (url.scheme() == "file") {
            const QString path = QFileInfo(url.toLocalFile()).canonicalFilePath();
            if (!path.isEmpty() && !path.startsWith(root_ + QDir::separator())) {
                qInfo() << "resource_blocked=outside_probe_root";
                return {};
            }
        }
        return QTextBrowser::loadResource(type, url);
    }

private:
    QString root_;
};

class ProbeWindow final : public QMainWindow {
public:
    explicit ProbeWindow(const QString &root, QWidget *parent = nullptr)
        : QMainWindow(parent), root_(root) {
        setWindowTitle(QStringLiteral("QTextBrowser mail renderer probe"));
        resize(1100, 760);
        auto *central = new QWidget(this);
        auto *outer = new QVBoxLayout(central);
        auto *top = new QHBoxLayout;
        top->addWidget(new QLabel(QStringLiteral("Corpus:")));
        status_ = new QLabel;
        top->addWidget(status_);
        previous_ = new QPushButton(QStringLiteral("Previous"));
        next_ = new QPushButton(QStringLiteral("Next"));
        top->addWidget(previous_); top->addWidget(next_);
        outer->addLayout(top);
        auto *split = new QHBoxLayout;
        list_ = new QListWidget;
        browser_ = new LocalTextBrowser(root_, central);
        browser_->setUndoRedoEnabled(false);
        browser_->setFocusPolicy(Qt::StrongFocus);
        split->addWidget(list_, 1); split->addWidget(browser_, 3);
        outer->addLayout(split, 1);
        setCentralWidget(central);
        for (const auto &file : QDir(root_).entryList({"*.html"}, QDir::Files, QDir::Name))
            list_->addItem(file);
        connect(list_, &QListWidget::currentRowChanged, this, [this](int row) { showRow(row); });
        connect(previous_, &QPushButton::clicked, this, [this] { list_->setCurrentRow(list_->currentRow() - 1); });
        connect(next_, &QPushButton::clicked, this, [this] { list_->setCurrentRow(list_->currentRow() + 1); });
        if (list_->count()) list_->setCurrentRow(0);
    }

private:
    void showRow(int row) {
        if (row < 0 || row >= list_->count()) return;
        const QString file = QDir(root_).filePath(list_->item(row)->text());
        QFile input(file);
        if (!input.open(QIODevice::ReadOnly)) return;
        browser_->document()->setBaseUrl(QUrl::fromLocalFile(file));
        browser_->setHtml(QString::fromUtf8(input.readAll()));
        list_->setFocus();
        status_->setText(QStringLiteral("%1 / %2 — %3 bytes").arg(row + 1).arg(list_->count()).arg(input.size()));
    }
    QString root_;
    QLabel *status_{};
    QPushButton *previous_{};
    QPushButton *next_{};
    QListWidget *list_{};
    LocalTextBrowser *browser_{};
};

int main(int argc, char **argv) {
    QApplication app(argc, argv);
    const QString root = app.arguments().value(1, QStringLiteral("/tmp/qt-textbrowser-corpus-2"));
    ProbeWindow window(root);
    window.show();
    qInfo().noquote() << "qt_widgets_probe=true javascript=false network=blocked"
                      << "files=" << window.findChildren<QListWidget *>().first()->count();
    if (app.arguments().contains(QStringLiteral("--smoke")))
        QTimer::singleShot(2500, &app, &QCoreApplication::quit);
    return app.exec();
}

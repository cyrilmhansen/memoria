#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mail_archive_experiment::app_config::{GmailSourceConfig, MemoriaConfig};
use mail_archive_experiment::gmail::{self, GmailError, GmailTransport, SyncProgress};
use mail_archive_experiment::html_preview::HtmlPreviewServer;
use mail_archive_experiment::i18n::{self, Language};
use mail_archive_experiment::{
    archive_summary, available_gmail_labels, discover_providers, export_message_eml,
    index_gmail_archive, list_attachments, parse_gmail_message, parse_search_date_ms,
    read_archived_raw, read_attachment, read_html_document, selected_provider, AttachmentFilter,
    AttachmentInfo, BackendKind, ExtractionProvider, GmailSearchIndex, ProviderAvailability,
    ProviderSelection, SearchRequest,
};
mod thumbnail;
use slint::{Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

struct TempAttachmentStore {
    dir: PathBuf,
    counter: AtomicU64,
}

#[derive(Default)]
struct SearchFilterState {
    available_labels: Vec<String>,
    selected_labels: Vec<String>,
}

impl TempAttachmentStore {
    fn new() -> Self {
        Self {
            dir: env::temp_dir().join(format!("memoria-attachments-{}", std::process::id())),
            counter: AtomicU64::new(0),
        }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        fs::create_dir_all(&self.dir)
            .map_err(|error| format!("création du fichier temporaire : {error}"))?;
        let serial = self.counter.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(format!("{serial}-{name}"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("création du fichier temporaire : {error}"))?;
        std::io::Write::write_all(&mut file, bytes)
            .map_err(|error| format!("écriture du fichier temporaire : {error}"))?;
        Ok(path)
    }
}

impl Drop for TempAttachmentStore {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

slint::include_modules!();

fn localized_ui(language: Language) -> UiText {
    let t = i18n::UiStrings::for_language(language);
    UiText {
        window_title: t.window_title.into(),
        main_accessible: t.main_accessible.into(),
        file: t.file.into(),
        new_archive: t.new_archive.into(),
        open_archive: t.open_archive.into(),
        recent_archives: t.recent_archives.into(),
        export_eml: t.export_eml.into(),
        export_displayed_eml: t.export_displayed_eml.into(),
        quit: t.quit.into(),
        archive: t.archive.into(),
        archive_state: t.archive_state.into(),
        sync_now: t.sync_now.into(),
        syncing: t.syncing.into(),
        add_gmail: t.add_gmail.into(),
        back_search: t.back_search.into(),
        search_menu: t.search_menu.into(),
        focus_search: t.focus_search.into(),
        clear_search: t.clear_search.into(),
        view: t.view.into(),
        zoom_out: t.zoom_out.into(),
        zoom_reset: t.zoom_reset.into(),
        zoom_in: t.zoom_in.into(),
        help: t.help.into(),
        about: t.about.into(),
        archive_choice: t.archive_choice.into(),
        setup_description: t.setup_description.into(),
        create_archive: t.create_archive.into(),
        archive_sync: t.archive_sync.into(),
        local_archive: t.local_archive.into(),
        gmail_source: t.gmail_source.into(),
        add_account: t.add_account.into(),
        content_extraction: t.content_extraction.into(),
        search_placeholder: t.search_placeholder.into(),
        search_accessible: t.search_accessible.into(),
        clear: t.clear.into(),
        filters: t.filters.into(),
        filters_accessible: t.filters_accessible.into(),
        from_placeholder: t.from_placeholder.into(),
        from_accessible: t.from_accessible.into(),
        to_placeholder: t.to_placeholder.into(),
        to_accessible: t.to_accessible.into(),
        since_placeholder: t.since_placeholder.into(),
        since_accessible: t.since_accessible.into(),
        until_placeholder: t.until_placeholder.into(),
        until_accessible: t.until_accessible.into(),
        attachment_filter_prefix: t.attachment_filter_prefix.into(),
        attachment_filter_accessible: t.attachment_filter_accessible.into(),
        mime_placeholder: t.mime_placeholder.into(),
        mime_accessible: t.mime_accessible.into(),
        reset_filters: t.reset_filters.into(),
        labels_prefix: t.labels_prefix.into(),
        empty_search: t.empty_search.into(),
        results_accessible: t.results_accessible.into(),
        reader_accessible: t.reader_accessible.into(),
        select_message: t.select_message.into(),
        open_html: t.open_html.into(),
        reduce_zoom: t.reduce_zoom.into(),
        reset_zoom: t.reset_zoom.into(),
        increase_zoom: t.increase_zoom.into(),
        body_placeholder: t.body_placeholder.into(),
        attachment_accessible_prefix: t.attachment_accessible_prefix.into(),
        preview: t.preview.into(),
        open: t.open.into(),
        save_as: t.save_as.into(),
        preview_accessible_prefix: t.preview_accessible_prefix.into(),
        preview_region_prefix: t.preview_region_prefix.into(),
        close: t.close.into(),
    }
}

fn archive_argument() -> Option<PathBuf> {
    let arguments: Vec<String> = env::args().collect();
    arguments
        .windows(2)
        .find(|pair| pair[0] == "--archive")
        .map(|pair| PathBuf::from(&pair[1]))
        .or_else(|| env::var_os("MAIL_ARCHIVE_PATH").map(PathBuf::from))
}

fn option_argument(name: &str) -> Option<String> {
    env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn default_token_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mail-archive-experiment/tokens")
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn open_with_system(path: &std::path::Path) -> Result<(), String> {
    open::that_detached(path).map_err(|error| format!("aucune application associée : {error}"))
}

fn open_url_with_system(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|error| format!("aucun navigateur associé : {error}"))
}

fn sync_progress_view(language: Language, progress: &SyncProgress) -> (String, f32, bool) {
    let fraction = progress
        .total
        .filter(|total| *total > 0)
        .map(|total| (progress.examined as f32 / total as f32).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let text = if let Some(total) = progress.total {
        i18n::sync_progress(
            language,
            progress.examined,
            Some(total),
            progress.new_messages,
            progress.network_bytes,
        )
    } else {
        i18n::sync_progress(
            language,
            progress.examined,
            None,
            progress.new_messages,
            progress.network_bytes,
        )
    };
    (text, fraction, progress.total.is_some())
}

fn attachment_filename(info: &AttachmentInfo) -> String {
    let mut value = info
        .filename
        .as_deref()
        .unwrap_or("")
        .replace(['/', '\\'], "_");
    value = value.replace("..", "_");
    value.retain(|character| !character.is_control() && !"<>:\"|?*".contains(character));
    value = value.trim_matches([' ', '.']).to_string();
    if value.is_empty() {
        let extension = match info.mime.as_str() {
            "application/pdf" => ".pdf",
            "image/jpeg" => ".jpg",
            "image/png" => ".png",
            "text/plain" => ".txt",
            _ => ".bin",
        };
        value = format!("attachment-{}{}", info.id, extension);
    }
    let reserved = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved_device = matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && reserved.as_bytes()[3].is_ascii_digit()
            && reserved.as_bytes()[3] != b'0');
    if reserved_device {
        value.insert(0, '_');
    }
    value
}

fn eml_filename(date: &str, subject: &str, doc_id: u64) -> String {
    let mut value = format!("{date}-{subject}")
        .replace(['/', '\\'], "_")
        .replace("..", "_");
    value.retain(|character| !character.is_control() && !"<>:\"|?*".contains(character));
    value = value
        .trim_matches([' ', '.', '-'])
        .chars()
        .take(96)
        .collect();
    if value.is_empty() {
        value = format!("message-{doc_id}");
    }
    let reserved = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(reserved.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && reserved.as_bytes()[3].is_ascii_digit()
            && reserved.as_bytes()[3] != b'0')
    {
        value.insert(0, '_');
    }
    format!("{value}.eml")
}

#[derive(Debug, Default, PartialEq, Eq)]
struct EmlBatchSummary {
    exported: usize,
    errors: usize,
}

fn next_eml_destination(
    directory: &Path,
    filename: &str,
    reserved: &mut HashSet<PathBuf>,
) -> PathBuf {
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("message");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("eml");
    let mut index = 1;
    loop {
        let candidate_name = if index == 1 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}-{index}.{extension}")
        };
        let candidate = directory.join(candidate_name);
        if !candidate.exists() && reserved.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn export_eml_batch(
    archive: &Path,
    messages: &[(u64, String, String)],
    destination: &Path,
) -> EmlBatchSummary {
    let mut summary = EmlBatchSummary::default();
    let mut reserved = HashSet::new();
    for &(doc_id, ref date, ref subject) in messages {
        let filename = eml_filename(date, subject, doc_id);
        let path = next_eml_destination(destination, &filename, &mut reserved);
        match export_message_eml(archive, doc_id, &path) {
            Ok(()) => summary.exported += 1,
            Err(_) => summary.errors += 1,
        }
    }
    summary
}

fn attachment_rows(infos: &[AttachmentInfo]) -> Vec<AttachmentRow> {
    infos
        .iter()
        .map(|info| AttachmentRow {
            id: info.id as i32,
            name: attachment_filename(info).into(),
            mime: info.mime.clone().into(),
            size: human_bytes(info.decoded_bytes).into(),
            previewable: info.mime == "application/pdf" || info.mime.starts_with("image/"),
        })
        .collect()
}

fn set_archive_summary(ui: &MailWindow, archive: &std::path::Path) {
    let language = Language::system();
    match archive_summary(archive) {
        Ok(summary) => {
            ui.set_archive_message_count(i18n::message_count(language, summary.messages).into());
            ui.set_archive_size(
                format!(
                    "{}: {}",
                    if language == Language::Fr {
                        "Taille physique"
                    } else {
                        "Physical size"
                    },
                    i18n::format_bytes(summary.archive_bytes)
                )
                .into(),
            );
            ui.set_archive_segments(format!("{} segments", summary.segments).into());
            ui.set_archive_catalog(
                format!(
                    "{}: {}",
                    if language == Language::Fr {
                        "Catalogue"
                    } else {
                        "Catalog"
                    },
                    i18n::format_bytes(summary.catalog_bytes)
                )
                .into(),
            );
            ui.set_archive_index(if summary.index_present {
                format!(
                    "Index de recherche : à jour · {}",
                    human_bytes(summary.index_bytes)
                )
                .into()
            } else {
                if language == Language::Fr {
                    "Index de recherche : non construit"
                } else {
                    "Search index: not built"
                }
                .into()
            });
        }
        Err(error) => {
            ui.set_archive_message_count(i18n::status(language, "archive-inaccessible").into());
            ui.set_archive_size(error.to_string().into());
            ui.set_archive_segments(String::new().into());
            ui.set_archive_catalog(String::new().into());
            ui.set_archive_index(i18n::status(language, "index-unavailable").into());
        }
    }
}

fn provider_display_name(language: Language, provider: &ExtractionProvider) -> String {
    match provider.id.as_str() {
        "memoria-text" => match language {
            Language::Fr => "Décodeur texte intégré Memoria".into(),
            Language::En => "Memoria built-in text decoder".into(),
        },
        "poppler-pdftotext" => "Poppler pdftotext".into(),
        "windows-ifilter" => match language {
            Language::Fr => "IFilter Windows enregistré".into(),
            Language::En => "Windows registered IFilter".into(),
        },
        _ => provider.display_name.clone(),
    }
}

fn provider_backend_name(language: Language, backend: BackendKind) -> &'static str {
    match (language, backend) {
        (Language::Fr, BackendKind::BuiltIn) => "intégré",
        (Language::En, BackendKind::BuiltIn) => "built-in",
        (Language::Fr, BackendKind::ExternalExecutable) => "exécutable externe",
        (Language::En, BackendKind::ExternalExecutable) => "external executable",
        (Language::Fr, BackendKind::WindowsIFilter) => "IFilter Windows",
        (Language::En, BackendKind::WindowsIFilter) => "Windows IFilter",
    }
}

fn provider_availability_name(
    language: Language,
    availability: ProviderAvailability,
) -> &'static str {
    match (language, availability) {
        (Language::Fr, ProviderAvailability::Available) => "disponible",
        (Language::En, ProviderAvailability::Available) => "available",
        (Language::Fr, ProviderAvailability::Unavailable) => "indisponible",
        (Language::En, ProviderAvailability::Unavailable) => "unavailable",
    }
}

fn selected_provider_name(language: Language, mime: &str) -> String {
    selected_provider(mime, &ProviderSelection::Automatic)
        .map(|provider| {
            format!(
                "{} ({})",
                provider.id.as_str(),
                provider_display_name(language, &provider)
            )
        })
        .unwrap_or_else(|| match language {
            Language::Fr => "aucun provider disponible".into(),
            Language::En => "no provider available".into(),
        })
}

fn extraction_provider_status(language: Language) -> String {
    let mut lines = discover_providers()
        .iter()
        .map(|provider| {
            let path = provider
                .executable_path
                .as_ref()
                .map(|path| format!(" · {}", path.display()))
                .unwrap_or_default();
            format!(
                "{} — {} · {} · {} · {}{}",
                provider.id.as_str(),
                provider_display_name(language, provider),
                provider_backend_name(language, provider.backend_kind),
                provider_availability_name(language, provider.availability),
                provider.supported_types.join(", "),
                path
            )
        })
        .collect::<Vec<_>>();
    let automatic = match language {
        Language::Fr => "Sélection automatique",
        Language::En => "Automatic selection",
    };
    lines.push(format!(
        "{automatic} · PDF: {} · DOCX: {}",
        selected_provider_name(language, "application/pdf"),
        selected_provider_name(
            language,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        )
    ));
    lines.join("\n")
}

fn friendly_gmail_error(error: &GmailError) -> String {
    match error {
        GmailError::Config(_) => "Source Gmail non configurée ou OAuth invalide".into(),
        GmailError::Http(status) => format!("Gmail a refusé la requête (HTTP {status})"),
        GmailError::HistoryExpired => {
            "L’historique Gmail a expiré ; une réconciliation est nécessaire".into()
        }
        GmailError::Json(_) => "Réponse Gmail illisible".into(),
        GmailError::Io(_) => "Impossible de lire ou d’écrire l’archive".into(),
        GmailError::Other(_) => "Erreur réseau ou Gmail pendant la synchronisation".into(),
    }
}

fn archive_is_valid(path: &std::path::Path) -> bool {
    path.is_dir() && path.join("metadata.sqlite").is_file() && path.join("archive").is_dir()
}

fn initialize_archive(path: &std::path::Path) -> Result<(), String> {
    if path.exists() {
        let mut entries = fs::read_dir(path).map_err(|error| error.to_string())?;
        if entries.next().is_some() {
            return Err("Le dossier choisi n’est pas vide ; choisissez un nouveau dossier".into());
        }
    }
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    fs::create_dir_all(path.join("archive")).map_err(|error| error.to_string())?;
    mail_archive_experiment::create_catalogue(&path.join("metadata.sqlite"))
        .map_err(|error| error.to_string())?;
    GmailSearchIndex::open(path).map_err(|error| error.to_string())?;
    index_gmail_archive(path).map_err(|error| error.to_string())?;
    Ok(())
}

fn source_key(email: Option<&str>, credentials: &std::path::Path) -> String {
    let stable = email
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| credentials.to_string_lossy().into_owned());
    format!("gmail:{}", blake3::hash(stable.as_bytes()).to_hex())
}

fn set_source_state(ui: &MailWindow, source: Option<&GmailSourceConfig>) {
    match source {
        Some(source) if PathBuf::from(&source.credentials_path).exists() => {
            ui.set_source_configured(true);
            ui.set_source_status(if let Some(email) = &source.display_email {
                format!("Compte Gmail autorisé · {email}").into()
            } else {
                "Compte Gmail autorisé".into()
            });
        }
        Some(_) => {
            ui.set_source_configured(false);
            ui.set_source_status("Source configurée · fichier credentials introuvable".into());
        }
        None => {
            ui.set_source_configured(false);
            ui.set_source_status("Aucune source de courrier configurée".into());
        }
    }
}

fn format_date(timestamp_ms: i64) -> String {
    if timestamp_ms <= 0 {
        return "date inconnue".into();
    }
    let days = timestamp_ms.div_euclid(86_400_000);
    let seconds = timestamp_ms.rem_euclid(86_400_000) / 1000;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}",
        hour = seconds / 3600,
        minute = (seconds % 3600) / 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

fn build_search_request(ui: &MailWindow, filters: &SearchFilterState) -> SearchRequest {
    let date_from = parse_search_date_ms(ui.get_date_from_filter().as_str());
    let date_to = parse_search_date_ms(ui.get_date_to_filter().as_str())
        .and_then(|value| value.checked_add(86_400_000));
    let attachment = match ui.get_attachment_filter().as_str() {
        "Avec" => AttachmentFilter::With,
        "Sans" => AttachmentFilter::Without,
        _ => AttachmentFilter::All,
    };
    SearchRequest {
        text: ui.get_query().to_string(),
        from: (!ui.get_from_filter().trim().is_empty()).then(|| ui.get_from_filter().to_string()),
        to: (!ui.get_to_filter().trim().is_empty()).then(|| ui.get_to_filter().to_string()),
        date_from,
        date_to,
        attachment,
        attachment_mime: (!ui.get_attachment_mime_filter().trim().is_empty())
            .then(|| ui.get_attachment_mime_filter().trim().to_ascii_lowercase()),
        labels: filters.selected_labels.clone(),
        limit: 50,
    }
}

fn set_label_options(ui: &MailWindow, filters: &SearchFilterState) {
    let options = filters
        .available_labels
        .iter()
        .map(|label| {
            if filters
                .selected_labels
                .iter()
                .any(|selected| selected == label)
            {
                format!("✓ {label}")
            } else {
                format!("□ {label}")
            }
        })
        .collect::<Vec<_>>();
    ui.set_label_options(ModelRc::new(VecModel::from(
        options
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    let mut active = filters.selected_labels.len();
    for value in [
        ui.get_from_filter().trim(),
        ui.get_to_filter().trim(),
        ui.get_date_from_filter().trim(),
        ui.get_date_to_filter().trim(),
        ui.get_attachment_mime_filter().trim(),
    ] {
        if !value.is_empty() {
            active += 1;
        }
    }
    if ui.get_attachment_filter() != "Toutes" {
        active += 1;
    }
    ui.set_active_filter_count(active as i32);
}

fn load_results(ui: &MailWindow, index: &GmailSearchIndex, request: &SearchRequest) {
    let started = Instant::now();
    clear_message(ui);
    if request.text.trim().is_empty() && !request.has_filters() {
        ui.set_results(ModelRc::new(VecModel::from(Vec::<SearchRow>::new())));
        ui.set_result_count(i18n::status(Language::system(), "no-search").into());
        ui.set_status(i18n::status(Language::system(), "type-search").into());
        ui.set_selected_index(-1);
        return;
    }
    match index.search_request(request) {
        Ok(results) => {
            let rows: Vec<SearchRow> = results
                .iter()
                .map(|result| SearchRow {
                    doc_id: result.doc_id as i32,
                    date: SharedString::from(format_date(result.timestamp)),
                    correspondent: SharedString::from(if result.sender.is_empty() {
                        result.recipients.clone()
                    } else {
                        result.sender.clone()
                    }),
                    subject: SharedString::from(if result.subject.is_empty() {
                        "(sans sujet)".to_string()
                    } else {
                        result.subject.clone()
                    }),
                    snippet: SharedString::from(result.snippet.clone()),
                    attachment: SharedString::from(if result.attachment_count > 0 {
                        format!("📎 {}", result.attachment_count)
                    } else {
                        String::new()
                    }),
                })
                .collect();
            ui.set_result_count(i18n::result_count(Language::system(), rows.len()).into());
            ui.set_results(ModelRc::new(VecModel::from(rows)));
            ui.set_selected_index(-1);
            ui.set_status(format!("Recherche en {} µs", started.elapsed().as_micros()).into());
        }
        Err(error) => {
            ui.set_results(ModelRc::new(VecModel::from(Vec::<SearchRow>::new())));
            ui.set_result_count(i18n::status(Language::system(), "search-error").into());
            ui.set_status(error.to_string().into());
        }
    }
}

fn refresh_results(
    ui: &MailWindow,
    index: &Rc<RefCell<Option<GmailSearchIndex>>>,
    filters: &Arc<Mutex<SearchFilterState>>,
) {
    if let Some(search_index) = index.borrow().as_ref() {
        let state = filters.lock().unwrap();
        let request = build_search_request(ui, &state);
        load_results(ui, search_index, &request);
    }
}

fn clear_message(ui: &MailWindow) {
    ui.set_message_date(SharedString::default());
    ui.set_message_from(SharedString::default());
    ui.set_message_to(SharedString::default());
    ui.set_message_subject(SharedString::default());
    ui.set_message_body(SharedString::default());
    ui.set_html_available(false);
    ui.set_message_attachments(SharedString::default());
    ui.set_attachment_rows(ModelRc::new(VecModel::from(Vec::<AttachmentRow>::new())));
    ui.set_preview_visible(false);
    ui.set_preview_image(slint::Image::default());
    ui.set_preview_title(SharedString::default());
    ui.set_preview_status(SharedString::default());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let requested_archive = archive_argument();
    let mut user_config = MemoriaConfig::load().unwrap_or_default();
    let candidate = requested_archive.clone().or_else(|| {
        user_config
            .default_archive
            .clone()
            .map(PathBuf::from)
            .filter(|path| archive_is_valid(path))
            .or_else(|| {
                user_config
                    .recent_archives
                    .iter()
                    .map(PathBuf::from)
                    .find(|path| archive_is_valid(path))
            })
    });
    let mut initial_setup_status = if requested_archive.is_some() {
        "L’archive indiquée est introuvable ou invalide.".into()
    } else if candidate.is_none() {
        "Choisissez une archive pour commencer.".into()
    } else {
        String::new()
    };
    let mut initial_archive = None;
    let mut initial_index = None;
    if let Some(path) = candidate {
        if archive_is_valid(&path) {
            match GmailSearchIndex::open(&path) {
                Ok(index) => {
                    let path = fs::canonicalize(&path).unwrap_or(path);
                    user_config.remember_archive(&path);
                    initial_archive = Some(path);
                    initial_index = Some(index);
                    let _ = user_config.save();
                }
                Err(error) => initial_setup_status = format!("Archive illisible : {error}"),
            }
        }
    }
    let cli_credentials = option_argument("--credentials").map(PathBuf::from);
    let cli_token_dir = option_argument("--token-dir")
        .map(PathBuf::from)
        .unwrap_or_else(default_token_dir);
    let cli_account = option_argument("--account");
    if let (Some(archive), Some(credentials), Some(account)) =
        (&initial_archive, cli_credentials, cli_account)
    {
        user_config.set_source(
            archive,
            GmailSourceConfig {
                credentials_path: credentials.to_string_lossy().into_owned(),
                token_dir: cli_token_dir.to_string_lossy().into_owned(),
                account_key: account,
                display_email: None,
            },
        );
        let _ = user_config.save();
    }
    let open_started = Instant::now();
    let index_open_us = open_started.elapsed().as_micros();
    if env::args().any(|argument| argument == "--benchmark") {
        let archive = initial_archive
            .as_ref()
            .ok_or("--benchmark nécessite une archive valide")?;
        let index = initial_index
            .as_ref()
            .ok_or("--benchmark nécessite un index lisible")?;
        let search_started = Instant::now();
        let results = index.search("invoice", 50)?;
        let search_us = search_started.elapsed().as_micros();
        let message_started = Instant::now();
        let parsed = results
            .first()
            .map(|result| {
                let raw = read_archived_raw(archive, result.doc_id)?;
                let message = parse_gmail_message(&raw, Vec::new())
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                Ok::<_, std::io::Error>((message, raw.len()))
            })
            .transpose()?;
        println!(
            "command=mail-archive-app-benchmark\nindex_open_us={}\nsearch_results={}\nsearch_us={}\nmessage_read_parse_us={}\nmessage_raw_bytes={}\nmessage_parse_ok={}",
            index_open_us,
            results.len(),
            search_us,
            message_started.elapsed().as_micros(),
            parsed.as_ref().map(|value| value.1).unwrap_or(0),
            parsed.is_some(),
        );
        return Ok(());
    }
    let ui = MailWindow::new()?;
    let language = Language::system();
    ui.set_texts(localized_ui(language));
    ui.set_extraction_provider_status(extraction_provider_status(language).into());
    ui.set_attachment_filter_label(i18n::attachment_filter_label(language, "Toutes").into());
    ui.set_status(format!("Index ouvert en {index_open_us} µs").into());
    ui.set_query(SharedString::default());
    clear_message(&ui);
    ui.set_result_count(i18n::status(language, "no-search").into());
    ui.set_status(i18n::status(language, "type-search").into());
    ui.set_setup_view(initial_index.is_none());
    ui.set_setup_status(initial_setup_status.into());
    let current_archive = Rc::new(RefCell::new(initial_archive));
    let current_index = Rc::new(RefCell::new(initial_index));
    let html_server = HtmlPreviewServer::start().ok().map(Arc::new);
    let search_filters = Arc::new(Mutex::new(SearchFilterState::default()));
    let config_state = Arc::new(Mutex::new(user_config));
    let current_source = Arc::new(Mutex::new(
        current_archive
            .borrow()
            .as_ref()
            .and_then(|path| config_state.lock().unwrap().source_for(path)),
    ));
    if let Some(archive) = current_archive.borrow().as_ref() {
        set_archive_summary(&ui, archive);
        if let Ok(labels) = available_gmail_labels(archive) {
            search_filters.lock().unwrap().available_labels = labels;
            set_label_options(&ui, &search_filters.lock().unwrap());
        }
        let source = current_source.lock().unwrap().clone();
        set_source_state(&ui, source.as_ref());
    } else {
        ui.set_source_status(i18n::status(language, "source-unconfigured").into());
        ui.set_archive_message_count(i18n::status(language, "no-archive").into());
    }

    let weak: Weak<MailWindow> = ui.as_weak();
    let search_index = current_index.clone();
    let filters = search_filters.clone();
    ui.on_search_changed(move |query| {
        if let Some(ui) = weak.upgrade() {
            ui.set_query(query.clone());
            refresh_results(&ui, &search_index, &filters);
        }
    });

    let weak = ui.as_weak();
    let search_index = current_index.clone();
    let filters = search_filters.clone();
    ui.on_search_submitted(move |query| {
        if let Some(ui) = weak.upgrade() {
            ui.set_query(query.clone());
            refresh_results(&ui, &search_index, &filters);
        }
    });

    let weak = ui.as_weak();
    let clear_search_index = current_index.clone();
    let clear_filters = search_filters.clone();
    ui.on_clear_search(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_query(SharedString::default());
            refresh_results(&ui, &clear_search_index, &clear_filters);
        }
    });

    let weak = ui.as_weak();
    ui.on_toggle_filters(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_filters_open(!ui.get_filters_open());
        }
    });

    let weak = ui.as_weak();
    let search_index = current_index.clone();
    let filters = search_filters.clone();
    ui.on_filters_changed(move || {
        if let Some(ui) = weak.upgrade() {
            set_label_options(&ui, &filters.lock().unwrap());
            refresh_results(&ui, &search_index, &filters);
        }
    });

    let weak = ui.as_weak();
    let search_index = current_index.clone();
    let filters = search_filters.clone();
    ui.on_reset_filters(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_from_filter(SharedString::default());
            ui.set_to_filter(SharedString::default());
            ui.set_date_from_filter(SharedString::default());
            ui.set_date_to_filter(SharedString::default());
            ui.set_attachment_mime_filter(SharedString::default());
            ui.set_attachment_filter("Toutes".into());
            ui.set_attachment_filter_label(
                i18n::attachment_filter_label(Language::system(), "Toutes").into(),
            );
            let mut state = filters.lock().unwrap();
            state.selected_labels.clear();
            set_label_options(&ui, &state);
            drop(state);
            refresh_results(&ui, &search_index, &filters);
        }
    });

    let weak = ui.as_weak();
    let search_index = current_index.clone();
    let filters = search_filters.clone();
    ui.on_cycle_attachment_filter(move || {
        if let Some(ui) = weak.upgrade() {
            let next = match ui.get_attachment_filter().as_str() {
                "Toutes" => "Avec",
                "Avec" => "Sans",
                _ => "Toutes",
            };
            ui.set_attachment_filter(next.into());
            ui.set_attachment_filter_label(
                i18n::attachment_filter_label(Language::system(), next).into(),
            );
            set_label_options(&ui, &filters.lock().unwrap());
            refresh_results(&ui, &search_index, &filters);
        }
    });

    let weak = ui.as_weak();
    let search_index = current_index.clone();
    let filters = search_filters.clone();
    ui.on_toggle_label(move |index| {
        if let Some(ui) = weak.upgrade() {
            let mut state = filters.lock().unwrap();
            if let Some(label) = state.available_labels.get(index as usize).cloned() {
                if let Some(position) = state.selected_labels.iter().position(|item| item == &label)
                {
                    state.selected_labels.remove(position);
                } else {
                    state.selected_labels.push(label);
                }
            }
            set_label_options(&ui, &state);
            drop(state);
            refresh_results(&ui, &search_index, &filters);
        }
    });

    let weak = ui.as_weak();
    ui.on_selection_move(move |delta| {
        if let Some(ui) = weak.upgrade() {
            let count = ui.get_results().row_count() as i32;
            if count == 0 {
                return;
            }
            let current = ui.get_selected_index();
            let next = (if current < 0 { 0 } else { current + delta }).clamp(0, count - 1);
            ui.set_selected_index(next);
        }
    });

    let weak = ui.as_weak();
    ui.on_zoom_change(move |delta| {
        if let Some(ui) = weak.upgrade() {
            let current = ui.get_message_font_size();
            let next = if delta == 0 {
                14.0
            } else {
                (current + (delta as f32 * 2.0)).clamp(10.0, 28.0)
            };
            ui.set_message_font_size(next);
            ui.set_message_zoom_label(format!("{} %", (next / 14.0 * 100.0).round() as i32).into());
            ui.set_status(
                format!(
                    "Zoom du message : {} %",
                    (next / 14.0 * 100.0).round() as i32
                )
                .into(),
            );
        }
    });

    let weak = ui.as_weak();
    ui.on_show_archive(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_archive_view(true);
        }
    });

    let weak = ui.as_weak();
    ui.on_show_search(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_archive_view(false);
        }
    });

    let sync_running = Arc::new(AtomicBool::new(false));
    let weak = ui.as_weak();
    let sync_running_for_handler = sync_running.clone();
    let archive_for_sync = current_archive.clone();
    let source_for_sync = current_source.clone();
    let index_for_refresh = current_index.clone();
    ui.on_refresh_search_index(move || {
        index_for_refresh
            .borrow()
            .as_ref()
            .map(|index| index.reload().is_ok())
            .unwrap_or(false)
    });
    ui.on_sync_requested(move || {
        let Some(ui) = weak.upgrade() else { return };
        if sync_running_for_handler.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(archive) = archive_for_sync.borrow().clone() else {
            sync_running_for_handler.store(false, Ordering::Release);
            ui.set_setup_status("Ouvrez ou créez une archive avant de synchroniser".into());
            return;
        };
        let Some(source) = source_for_sync.lock().unwrap().clone() else {
            sync_running_for_handler.store(false, Ordering::Release);
            ui.set_source_status("Aucune source Gmail configurée".into());
            return;
        };
        let credentials = PathBuf::from(&source.credentials_path);
        let token_dir = PathBuf::from(&source.token_dir);
        if token_dir.starts_with(&archive) || credentials.starts_with(&archive) {
            sync_running_for_handler.store(false, Ordering::Release);
            ui.set_source_status("Credentials et tokens doivent rester hors de l’archive".into());
            return;
        }
        ui.set_syncing(true);
        ui.set_sync_current(0);
        ui.set_sync_total(-1);
        ui.set_sync_fraction(0.0);
        ui.set_sync_determinate(false);
        ui.set_source_status(i18n::status(Language::system(), "sync-running").into());
        ui.set_sync_progress(i18n::status(Language::system(), "prepare-sync").into());
        let account = source.account_key;
        let running = sync_running_for_handler.clone();
        let weak = ui.as_weak();
        thread::spawn(move || {
            let result = (|| -> Result<gmail::SyncStats, String> {
                let mut transport = gmail::HttpGmail::authenticate(&credentials, &token_dir)
                    .map_err(|error| friendly_gmail_error(&error))?;
                let progress_weak = weak.clone();
                let stats = gmail::sync_account_with_progress(
                    &archive,
                    &account,
                    &mut transport,
                    None,
                    None,
                    move |progress: &SyncProgress| {
                        let snapshot = progress.clone();
                        let weak = progress_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = weak.upgrade() {
                                ui.set_source_status(
                                    i18n::status(Language::system(), "sync-running").into(),
                                );
                                ui.set_sync_current(snapshot.examined as i32);
                                ui.set_sync_total(
                                    snapshot.total.map(|value| value as i32).unwrap_or(-1),
                                );
                                let (progress_text, fraction, determinate) =
                                    sync_progress_view(language, &snapshot);
                                ui.set_sync_determinate(determinate);
                                ui.set_sync_fraction(fraction);
                                ui.set_sync_progress(progress_text.into());
                            }
                        });
                    },
                )
                .map_err(|error| friendly_gmail_error(&error))?;
                let indexing_weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = indexing_weak.upgrade() {
                        ui.set_source_status(i18n::status(Language::system(), "indexing").into());
                        ui.set_sync_progress(
                            i18n::status(Language::system(), "index-ready").into(),
                        );
                    }
                });
                index_gmail_archive(&archive)
                    .map_err(|_| i18n::status(Language::system(), "index-failed"))?;
                Ok(stats)
            })();
            let _ = slint::invoke_from_event_loop(move || {
                running.store(false, Ordering::Release);
                if let Some(ui) = weak.upgrade() {
                    ui.set_syncing(false);
                    match result {
                        Ok(stats) => {
                            if let Some(total) = stats.total {
                                ui.set_sync_current(total as i32);
                                ui.set_sync_total(total as i32);
                                ui.set_sync_fraction(1.0);
                                ui.set_sync_determinate(true);
                            }
                            let index_ok = ui.invoke_refresh_search_index();
                            set_archive_summary(&ui, &archive);
                            if index_ok {
                                ui.set_source_status(
                                    i18n::status(Language::system(), "archive-up-to-date").into(),
                                );
                                ui.set_sync_progress(if stats.new_messages == 0 {
                                    i18n::status(Language::system(), "no-new").into()
                                } else {
                                    i18n::sync_finished(
                                        Language::system(),
                                        stats.new_messages,
                                        stats.archive_bytes_added,
                                    )
                                    .into()
                                });
                            } else {
                                ui.set_source_status(
                                    i18n::status(Language::system(), "index-failed").into(),
                                );
                                ui.set_sync_progress(
                                    i18n::status(Language::system(), "index-failed").into(),
                                );
                            }
                        }
                        Err(error) => {
                            set_archive_summary(&ui, &archive);
                            ui.set_source_status(error.into());
                            ui.set_sync_progress(
                                i18n::status(Language::system(), "search-available").into(),
                            );
                        }
                    }
                }
            });
        });
    });

    let selection_generation = Arc::new(AtomicU64::new(0));
    let selected_doc_id = Arc::new(AtomicU64::new(0));
    let temp_attachments = Arc::new(TempAttachmentStore::new());
    let weak = ui.as_weak();
    let archive_for_selection = current_archive.clone();
    let selected_doc_for_selection = selected_doc_id.clone();
    ui.on_result_selected(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(row) = ui.get_results().row_data(index as usize) else {
            return;
        };
        let generation = selection_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let generation_counter = selection_generation.clone();
        let Some(archive) = archive_for_selection.borrow().clone() else {
            ui.set_status("Aucune archive ouverte".into());
            return;
        };
        ui.set_selected_index(index);
        selected_doc_for_selection.store(row.doc_id as u64, Ordering::Release);
        ui.set_loading(true);
        ui.set_status("Lecture du message…".into());
        let weak = ui.as_weak();
        thread::spawn(move || {
            let started = Instant::now();
            let result = read_archived_raw(&archive, row.doc_id as u64)
                .map_err(|error| error.to_string())
                .and_then(|raw| {
                    let parsed =
                        parse_gmail_message(&raw, Vec::new()).map_err(|error| error.to_string())?;
                    let attachments =
                        list_attachments(&archive, row.doc_id as u64).unwrap_or_default();
                    let html_available = read_html_document(&archive, row.doc_id as u64)
                        .map(|document| document.is_some())
                        .unwrap_or(false);
                    Ok((parsed, raw.len(), attachments, html_available))
                });
            let _ = slint::invoke_from_event_loop(move || {
                if generation_counter.load(Ordering::Relaxed) != generation {
                    return;
                }
                if let Some(ui) = weak.upgrade() {
                    ui.set_loading(false);
                    match result {
                        Ok((message, raw_bytes, attachments, html_available)) => {
                            ui.set_message_date(row.date.clone());
                            ui.set_message_from(format!("From: {}", message.sender).into());
                            ui.set_message_to(format!("To: {}", message.recipients).into());
                            ui.set_message_subject(message.subject.into());
                            ui.set_message_body(message.body.into());
                            ui.set_html_available(html_available);
                            ui.set_message_attachments(if message.attachment_count > 0 {
                                format!(
                                    "Pièces jointes : {} · RAW : {} octets",
                                    message.attachment_count, raw_bytes
                                )
                                .into()
                            } else {
                                format!("RAW : {} octets", raw_bytes).into()
                            });
                            ui.set_attachment_rows(ModelRc::new(VecModel::from(attachment_rows(
                                &attachments,
                            ))));
                            ui.set_status(
                                format!("Message affiché en {} µs", started.elapsed().as_micros())
                                    .into(),
                            );
                        }
                        Err(error) => {
                            ui.set_html_available(false);
                            ui.set_message_body(
                                format!("Impossible d’afficher ce message : {error}").into(),
                            );
                            ui.set_status("Erreur de lecture".into());
                        }
                    }
                }
            });
        });
    });

    let weak = ui.as_weak();
    let archive_for_html = current_archive.clone();
    let selected_for_html = selected_doc_id.clone();
    let html_server_for_open = html_server.clone();
    ui.on_open_html(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_for_html.borrow().clone() else {
            ui.set_status("Aucune archive ouverte".into());
            return;
        };
        let doc_id = selected_for_html.load(Ordering::Acquire);
        if doc_id == 0 {
            ui.set_status("Aucun message sélectionné".into());
            return;
        }
        let Some(server) = html_server_for_open.clone() else {
            ui.set_status("Serveur HTML local indisponible".into());
            return;
        };
        ui.set_status("Ouverture du message HTML…".into());
        let weak = ui.as_weak();
        thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                server
                    .open_message(&archive, doc_id)
                    .map_err(|error| format!("préparation HTML impossible : {error}"))?
                    .ok_or_else(|| "aucune partie HTML dans ce message".to_string())
                    .and_then(|url| open_url_with_system(&url).map(|()| url))
            })();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status(match result {
                        Ok(_) => "Message HTML ouvert dans le navigateur".into(),
                        Err(error) => error.into(),
                    });
                }
            });
        });
    });

    let weak = ui.as_weak();
    let archive_for_export_eml = current_archive.clone();
    let selected_for_export_eml = selected_doc_id.clone();
    let export_language = language;
    ui.on_export_eml_requested(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_for_export_eml.borrow().clone() else {
            ui.set_status(i18n::status(export_language, "archive-inaccessible").into());
            return;
        };
        let doc_id = selected_for_export_eml.load(Ordering::Acquire);
        if doc_id == 0 {
            ui.set_status(i18n::status(export_language, "no-selected").into());
            return;
        }
        let filename = eml_filename(
            ui.get_message_date().as_str(),
            ui.get_message_subject().as_str(),
            doc_id,
        );
        let strings = i18n::UiStrings::for_language(export_language);
        let Some(destination) = rfd::FileDialog::new()
            .set_title(strings.export_eml)
            .set_file_name(&filename)
            .save_file()
        else {
            ui.set_status(i18n::status(export_language, "cancelled").into());
            return;
        };
        let weak = ui.as_weak();
        thread::spawn(move || {
            let result = export_message_eml(&archive, doc_id, &destination)
                .map_err(|error| error.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status(match result {
                        Ok(()) => i18n::status(export_language, "eml-exported").into(),
                        Err(error) => format!(
                            "{}: {error}",
                            i18n::status(export_language, "eml-export-failed")
                        )
                        .into(),
                    });
                }
            });
        });
    });

    let weak = ui.as_weak();
    let archive_for_export_displayed_eml = current_archive.clone();
    let export_results_language = language;
    ui.on_export_results_eml_requested(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_for_export_displayed_eml.borrow().clone() else {
            ui.set_status(i18n::status(export_results_language, "archive-inaccessible").into());
            return;
        };
        let messages = (0..ui.get_results().row_count())
            .filter_map(|index| ui.get_results().row_data(index))
            .map(|row| {
                (
                    row.doc_id as u64,
                    row.date.to_string(),
                    row.subject.to_string(),
                )
            })
            .collect::<Vec<_>>();
        if messages.is_empty() {
            ui.set_status(i18n::status(export_results_language, "no-results").into());
            return;
        }
        let strings = i18n::UiStrings::for_language(export_results_language);
        let Some(destination) = rfd::FileDialog::new()
            .set_title(strings.export_displayed_eml)
            .pick_folder()
        else {
            ui.set_status(i18n::status(export_results_language, "cancelled").into());
            return;
        };
        ui.set_status(i18n::status(export_results_language, "eml-batch-started").into());
        let weak = ui.as_weak();
        thread::spawn(move || {
            let summary = export_eml_batch(&archive, &messages, &destination);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status(
                        i18n::eml_batch_finished(
                            export_results_language,
                            messages.len(),
                            summary.exported,
                            summary.errors,
                        )
                        .into(),
                    );
                }
            });
        });
    });

    let weak = ui.as_weak();
    let archive_for_open_attachment = current_archive.clone();
    let selected_for_open_attachment = selected_doc_id.clone();
    let temp_for_open_attachment = temp_attachments.clone();
    ui.on_open_attachment(move |attachment_id| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_for_open_attachment.borrow().clone() else {
            ui.set_status("Aucune archive ouverte".into());
            return;
        };
        let doc_id = selected_for_open_attachment.load(Ordering::Acquire);
        if doc_id == 0 {
            ui.set_status("Aucun message sélectionné".into());
            return;
        }
        let info = match list_attachments(&archive, doc_id) {
            Ok(items) => items
                .into_iter()
                .find(|item| item.id == attachment_id as u32),
            Err(_) => None,
        };
        let Some(info) = info else {
            ui.set_status("Pièce jointe introuvable ou MIME invalide".into());
            return;
        };
        let name = attachment_filename(&info);
        ui.set_status("Préparation de la pièce jointe…".into());
        let weak = ui.as_weak();
        let temp_store = temp_for_open_attachment.clone();
        thread::spawn(move || {
            let result = read_attachment(&archive, doc_id, attachment_id as u32)
                .map_err(|error| format!("contenu impossible à décoder : {error}"))
                .and_then(|bytes| {
                    let path = temp_store.write(&name, &bytes)?;
                    open_with_system(&path)?;
                    Ok(())
                });
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status(match result {
                        Ok(()) => "Pièce jointe ouverte avec l’application système".into(),
                        Err(error) => error.into(),
                    });
                }
            });
        });
    });

    let weak = ui.as_weak();
    let archive_for_save_attachment = current_archive.clone();
    let selected_for_save_attachment = selected_doc_id.clone();
    ui.on_save_attachment(move |attachment_id| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_for_save_attachment.borrow().clone() else {
            ui.set_status("Aucune archive ouverte".into());
            return;
        };
        let doc_id = selected_for_save_attachment.load(Ordering::Acquire);
        let Ok(items) = list_attachments(&archive, doc_id) else {
            ui.set_status("MIME invalide : pièces jointes indisponibles".into());
            return;
        };
        let Some(info) = items
            .into_iter()
            .find(|item| item.id == attachment_id as u32)
        else {
            ui.set_status("Pièce jointe introuvable".into());
            return;
        };
        let name = attachment_filename(&info);
        let Some(destination) = rfd::FileDialog::new()
            .set_title("Enregistrer la pièce jointe")
            .set_file_name(&name)
            .save_file()
        else {
            ui.set_status("Enregistrement annulé".into());
            return;
        };
        let weak = ui.as_weak();
        thread::spawn(move || {
            let result = read_attachment(&archive, doc_id, attachment_id as u32)
                .map_err(|error| format!("contenu impossible à décoder : {error}"))
                .and_then(|bytes| {
                    fs::write(&destination, bytes)
                        .map_err(|error| format!("écriture impossible : {error}"))
                });
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status(match result {
                        Ok(()) => "Pièce jointe enregistrée".into(),
                        Err(error) => error.into(),
                    });
                }
            });
        });
    });

    let weak = ui.as_weak();
    let archive_for_preview = current_archive.clone();
    let selected_for_preview = selected_doc_id.clone();
    let temp_for_preview = temp_attachments.clone();
    ui.on_preview_attachment(move |attachment_id| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_for_preview.borrow().clone() else {
            ui.set_status("Aucune archive ouverte".into());
            return;
        };
        let doc_id = selected_for_preview.load(Ordering::Acquire);
        if doc_id == 0 {
            ui.set_status("Aucun message sélectionné".into());
            return;
        }
        let info = match list_attachments(&archive, doc_id) {
            Ok(items) => items
                .into_iter()
                .find(|item| item.id == attachment_id as u32),
            Err(_) => None,
        };
        let Some(info) = info else {
            ui.set_status("Pièce jointe introuvable ou MIME invalide".into());
            return;
        };
        if !(info.mime == "application/pdf" || info.mime.starts_with("image/")) {
            ui.set_status("Aperçu indisponible pour ce type de pièce jointe".into());
            return;
        }
        let name = attachment_filename(&info);
        let title = name.clone();
        let temp_store = temp_for_preview.clone();
        ui.set_preview_title(title.into());
        ui.set_preview_image(slint::Image::default());
        ui.set_preview_status("Génération de l’aperçu…".into());
        ui.set_preview_visible(true);
        let weak = ui.as_weak();
        thread::spawn(move || {
            let result = read_attachment(&archive, doc_id, attachment_id as u32)
                .map_err(|error| format!("contenu impossible à décoder : {error}"))
                .and_then(|bytes| {
                    let input = temp_store.write(&name, &bytes)?;
                    let output = thumbnail::preview_attachment(&input, &temp_store.dir, 900)
                        .map_err(|error| error.to_string())?;
                    Ok::<PathBuf, String>(output)
                });
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(output) => match slint::Image::load_from_path(&output) {
                            Ok(image) => {
                                ui.set_preview_image(image);
                                ui.set_preview_status(SharedString::default());
                            }
                            Err(error) => {
                                ui.set_preview_visible(false);
                                ui.set_preview_status(
                                    format!("Aperçu indisponible : PNG illisible ({error})").into(),
                                );
                            }
                        },
                        Err(error) => {
                            ui.set_preview_visible(false);
                            ui.set_preview_status(format!("Aperçu indisponible : {error}").into());
                            ui.set_status(
                                "La pièce jointe reste disponible via Ouvrir ou Enregistrer sous…"
                                    .into(),
                            );
                        }
                    }
                }
            });
        });
    });

    let weak = ui.as_weak();
    ui.on_close_preview(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_preview_visible(false);
            ui.set_preview_status(SharedString::default());
            ui.set_preview_image(slint::Image::default());
        }
    });

    let weak = ui.as_weak();
    let archive_state = current_archive.clone();
    let index_state = current_index.clone();
    let config_state_for_open = config_state.clone();
    let source_state = current_source.clone();
    ui.on_open_archive_requested(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Ouvrir une archive Memoria")
            .pick_folder()
        else {
            return;
        };
        let Some(ui) = weak.upgrade() else { return };
        if !archive_is_valid(&path) {
            ui.set_setup_status("Ce dossier ne contient pas une archive Memoria valide".into());
            return;
        }
        match GmailSearchIndex::open(&path) {
            Ok(index) => {
                let path = fs::canonicalize(&path).unwrap_or(path);
                archive_state.replace(Some(path.clone()));
                index_state.replace(Some(index));
                let source = config_state_for_open.lock().unwrap().source_for(&path);
                *source_state.lock().unwrap() = source.clone();
                {
                    let mut config = config_state_for_open.lock().unwrap();
                    config.remember_archive(&path);
                    let _ = config.save();
                }
                ui.set_setup_view(false);
                ui.set_archive_view(false);
                ui.set_setup_status(SharedString::default());
                set_archive_summary(&ui, &path);
                set_source_state(&ui, source.as_ref());
                ui.set_status("Archive ouverte".into());
            }
            Err(error) => ui.set_setup_status(format!("Archive illisible : {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let archive_state = current_archive.clone();
    let index_state = current_index.clone();
    let config_state_for_create = config_state.clone();
    let source_state = current_source.clone();
    ui.on_create_archive_requested(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Créer une archive Memoria")
            .pick_folder()
        else {
            return;
        };
        let Some(ui) = weak.upgrade() else { return };
        match initialize_archive(&path) {
            Ok(()) => match GmailSearchIndex::open(&path) {
                Ok(index) => {
                    let path = fs::canonicalize(&path).unwrap_or(path);
                    archive_state.replace(Some(path.clone()));
                    index_state.replace(Some(index));
                    *source_state.lock().unwrap() = None;
                    let mut config = config_state_for_create.lock().unwrap();
                    config.remember_archive(&path);
                    let _ = config.save();
                    ui.set_setup_view(false);
                    ui.set_archive_view(false);
                    set_archive_summary(&ui, &path);
                    set_source_state(&ui, None);
                    ui.set_status("Nouvelle archive créée".into());
                }
                Err(error) => ui.set_setup_status(
                    format!("Archive créée mais index indisponible : {error}").into(),
                ),
            },
            Err(error) => ui.set_setup_status(error.into()),
        }
    });

    let weak = ui.as_weak();
    let archive_state = current_archive.clone();
    let source_state = current_source.clone();
    let config_state = config_state.clone();
    let cli_credentials = option_argument("--credentials").map(PathBuf::from);
    let cli_token_dir = option_argument("--token-dir").map(PathBuf::from);
    ui.on_add_source_requested(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(archive) = archive_state.borrow().clone() else {
            ui.set_setup_status("Ouvrez ou créez une archive avant d’ajouter Gmail".into());
            return;
        };
        let credentials = cli_credentials.clone().or_else(|| {
            rfd::FileDialog::new()
                .set_title("Choisir le fichier credentials Google")
                .add_filter("JSON", &["json"])
                .pick_file()
        });
        let Some(credentials) = credentials else {
            ui.set_source_status("Autorisation annulée".into());
            return;
        };
        let token_dir = cli_token_dir.clone().unwrap_or_else(default_token_dir);
        if credentials.starts_with(&archive) || token_dir.starts_with(&archive) {
            ui.set_source_status("Credentials et tokens doivent rester hors de l’archive".into());
            return;
        }
        ui.set_source_configured(false);
        ui.set_source_status("Ouverture du navigateur…".into());
        let weak = ui.as_weak();
        let source_state = source_state.clone();
        let config_state = config_state.clone();
        thread::spawn(move || {
            let result = (|| -> Result<GmailSourceConfig, String> {
                let mut transport = gmail::HttpGmail::authenticate(&credentials, &token_dir)
                    .map_err(|error| friendly_gmail_error(&error))?;
                let profile = transport
                    .profile()
                    .map_err(|error| friendly_gmail_error(&error))?;
                let email = profile.email_address.clone();
                Ok(GmailSourceConfig {
                    credentials_path: credentials.to_string_lossy().into_owned(),
                    token_dir: token_dir.to_string_lossy().into_owned(),
                    account_key: source_key(email.as_deref(), &credentials),
                    display_email: email,
                })
            })();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    match result {
                        Ok(source) => {
                            config_state
                                .lock()
                                .unwrap()
                                .set_source(&archive, source.clone());
                            let _ = config_state.lock().unwrap().save();
                            *source_state.lock().unwrap() = Some(source.clone());
                            set_source_state(&ui, Some(&source));
                            ui.set_source_status(
                                "Compte Gmail autorisé · choisissez Synchroniser maintenant".into(),
                            );
                        }
                        Err(error) => {
                            ui.set_source_status(
                                format!("Autorisation refusée ou configuration invalide : {error}")
                                    .into(),
                            );
                        }
                    }
                }
            });
        });
    });

    ui.on_quit_requested(move || {
        let _ = slint::quit_event_loop();
    });

    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_validation_and_creation_are_explicit() {
        let root = std::env::temp_dir().join(format!(
            "memoria-app-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        assert!(!archive_is_valid(&root));
        initialize_archive(&root).unwrap();
        assert!(archive_is_valid(&root));
        let invalid = root.join("invalid");
        fs::create_dir_all(&invalid).unwrap();
        fs::write(invalid.join("note"), b"not an archive").unwrap();
        assert!(!archive_is_valid(&invalid));
        assert!(initialize_archive(&invalid).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attachment_names_are_safe_for_native_save_dialogs() {
        for filename in [
            "CON",
            "CON.pdf",
            "PRN.txt",
            "AUX.jpg",
            "NUL",
            "COM1.dat",
            "COM9",
            "LPT1.txt",
            "LPT9.png",
            "../foo.pdf",
            r"..\foo.pdf",
            "trailing-space ",
            "trailing-dot.",
        ] {
            let info = AttachmentInfo {
                id: 3,
                filename: Some(filename.into()),
                mime: "application/pdf".into(),
                decoded_bytes: 10,
                content_id: None,
                inline: false,
            };
            let name = attachment_filename(&info);
            assert!(!name.contains(".."), "{filename} -> {name}");
            assert!(!name.contains('/'), "{filename} -> {name}");
            assert!(!name.contains('\\'), "{filename} -> {name}");
            assert!(!name.ends_with(' '), "{filename} -> {name}");
            assert!(!name.ends_with('.'), "{filename} -> {name}");
            let stem = name
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            assert!(!matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL"));
            assert!(
                !(stem.starts_with("COM") || stem.starts_with("LPT"))
                    || !matches!(stem.as_bytes().get(3), Some(b'1'..=b'9'))
            );
        }
    }

    #[test]
    fn eml_export_names_are_safe_and_have_a_stable_fallback() {
        let name = eml_filename("2026-08-22 10:20", "Sujet: résumé/essai", 42);
        assert_eq!(name, "2026-08-22 1020-Sujet résumé_essai.eml");
        assert_eq!(eml_filename("", "", 42), "message-42.eml");
        assert!(!eml_filename("", "CON", 42).starts_with("CON."));
    }

    #[test]
    fn batch_eml_export_is_byte_exact_and_continues_after_errors() {
        let root = std::env::temp_dir().join(format!(
            "memoria-batch-eml-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let destination = root.join("exports");
        fs::create_dir_all(&destination).unwrap();
        let catalog =
            mail_archive_experiment::create_metadata(&root.join("metadata.sqlite")).unwrap();
        let mut writer =
            mail_archive_experiment::ArchiveWriter::open(&root.join("archive"), 4096).unwrap();
        let mut raws = Vec::new();
        for (id, subject) in [
            (0_u64, "same subject"),
            (1_u64, "same subject"),
            (2_u64, ""),
            (3_u64, "bad<>:/subject"),
        ] {
            let raw =
                format!("From: fixture@example.test\r\nSubject: {subject}\r\n\r\nbody-{id}\r\n")
                    .into_bytes();
            let message = mail_archive_experiment::Message {
                id,
                message_id: format!("batch-{id}"),
                timestamp: 0,
                sender: "fixture@example.test".into(),
                recipients: Vec::new(),
                subject: subject.into(),
                text_body: format!("body-{id}"),
                html_body: None,
                account: "fixture".into(),
                folder: "Inbox".into(),
                thread: "thread".into(),
                attachments: Vec::new(),
                raw: raw.clone(),
            };
            let location = writer.append(&message).unwrap();
            mail_archive_experiment::insert_metadata(&catalog, &message, &location).unwrap();
            raws.push(raw);
        }
        writer.sync().unwrap();
        drop(writer);
        drop(catalog);

        let existing = destination.join("date-same subject.eml");
        fs::write(&existing, b"must-not-be-overwritten").unwrap();
        let items = vec![
            (0, "date".into(), "same subject".into()),
            (1, "date".into(), "same subject".into()),
            (2, "date".into(), "".into()),
            (3, "date".into(), "bad<>:/subject".into()),
            (99, "date".into(), "missing".into()),
        ];
        let summary = export_eml_batch(&root, &items, &destination);
        assert_eq!(
            summary,
            EmlBatchSummary {
                exported: 4,
                errors: 1
            }
        );
        assert_eq!(fs::read(&existing).unwrap(), b"must-not-be-overwritten");
        assert_eq!(
            fs::read(destination.join("date-same subject-2.eml")).unwrap(),
            raws[0]
        );
        assert_eq!(
            fs::read(destination.join("date-same subject-3.eml")).unwrap(),
            raws[1]
        );
        assert_eq!(fs::read(destination.join("date.eml")).unwrap(), raws[2]);
        assert_eq!(
            fs::read(destination.join("date-bad_subject.eml")).unwrap(),
            raws[3]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_status_exposes_ids_and_automatic_document_selection() {
        let status = extraction_provider_status(Language::En);
        assert!(status.contains("memoria-text"));
        assert!(status.contains("poppler-pdftotext"));
        assert!(status.contains("Automatic selection"));
        assert!(status.contains("PDF:"));
        assert!(status.contains("DOCX:"));
    }

    #[test]
    fn sync_progress_view_distinguishes_known_and_unknown_totals() {
        let known = SyncProgress {
            examined: 0,
            total: Some(10),
            ..Default::default()
        };
        let (text, fraction, determinate) = sync_progress_view(Language::Fr, &known);
        assert!(text.contains("0 messages sur 10"));
        assert_eq!(fraction, 0.0);
        assert!(determinate);

        let complete = SyncProgress {
            examined: 10,
            total: Some(10),
            new_messages: 2,
            ..Default::default()
        };
        let (_, fraction, determinate) = sync_progress_view(Language::Fr, &complete);
        assert_eq!(fraction, 1.0);
        assert!(determinate);

        let unknown = SyncProgress {
            examined: 4,
            new_messages: 1,
            ..Default::default()
        };
        let (text, fraction, determinate) = sync_progress_view(Language::Fr, &unknown);
        assert!(text.contains("4 messages examinés"));
        assert_eq!(fraction, 0.0);
        assert!(!determinate);
    }
}

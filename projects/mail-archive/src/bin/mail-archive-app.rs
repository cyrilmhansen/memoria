#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mail_archive_experiment::app_config::{GmailSourceConfig, MemoriaConfig};
use mail_archive_experiment::gmail::{self, GmailError, GmailTransport, SyncProgress};
use mail_archive_experiment::{
    archive_summary, index_gmail_archive, parse_gmail_message, read_archived_raw, GmailSearchIndex,
};
use slint::{Model, ModelRc, SharedString, VecModel, Weak};
use std::cell::RefCell;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

slint::include_modules!();

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

fn set_archive_summary(ui: &MailWindow, archive: &std::path::Path) {
    match archive_summary(archive) {
        Ok(summary) => {
            ui.set_archive_message_count(format!("{} messages", summary.messages).into());
            ui.set_archive_size(
                format!("Taille physique : {}", human_bytes(summary.archive_bytes)).into(),
            );
            ui.set_archive_segments(format!("{} segments", summary.segments).into());
            ui.set_archive_catalog(
                format!("Catalogue : {}", human_bytes(summary.catalog_bytes)).into(),
            );
            ui.set_archive_index(if summary.index_present {
                format!(
                    "Index de recherche : à jour · {}",
                    human_bytes(summary.index_bytes)
                )
                .into()
            } else {
                "Index de recherche : non construit".into()
            });
        }
        Err(error) => {
            ui.set_archive_message_count("Archive inaccessible".into());
            ui.set_archive_size(error.to_string().into());
            ui.set_archive_segments(String::new().into());
            ui.set_archive_catalog(String::new().into());
            ui.set_archive_index("Index de recherche indisponible".into());
        }
    }
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
    mail_archive_experiment::create_metadata(&path.join("metadata.sqlite"))
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

fn load_results(ui: &MailWindow, index: &GmailSearchIndex, query: &str) {
    let started = Instant::now();
    clear_message(ui);
    if query.trim().is_empty() {
        ui.set_results(ModelRc::new(VecModel::from(Vec::<SearchRow>::new())));
        ui.set_result_count("Aucune recherche".into());
        ui.set_status("Saisissez une recherche pour commencer".into());
        ui.set_selected_index(-1);
        return;
    }
    match index.search(query, 50) {
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
            ui.set_result_count(format!("{} résultats", rows.len()).into());
            ui.set_results(ModelRc::new(VecModel::from(rows)));
            ui.set_selected_index(-1);
            ui.set_status(format!("Recherche en {} µs", started.elapsed().as_micros()).into());
        }
        Err(error) => {
            ui.set_results(ModelRc::new(VecModel::from(Vec::<SearchRow>::new())));
            ui.set_result_count("Erreur de recherche".into());
            ui.set_status(error.to_string().into());
        }
    }
}

fn clear_message(ui: &MailWindow) {
    ui.set_message_date(SharedString::default());
    ui.set_message_from(SharedString::default());
    ui.set_message_to(SharedString::default());
    ui.set_message_subject(SharedString::default());
    ui.set_message_body(SharedString::default());
    ui.set_message_attachments(SharedString::default());
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
    ui.set_status(format!("Index ouvert en {index_open_us} µs").into());
    ui.set_query(SharedString::default());
    clear_message(&ui);
    ui.set_result_count("Aucune recherche".into());
    ui.set_status("Saisissez une recherche pour commencer".into());
    ui.set_setup_view(initial_index.is_none());
    ui.set_setup_status(initial_setup_status.into());
    let current_archive = Rc::new(RefCell::new(initial_archive));
    let current_index = Rc::new(RefCell::new(initial_index));
    let config_state = Arc::new(Mutex::new(user_config));
    let current_source = Arc::new(Mutex::new(
        current_archive
            .borrow()
            .as_ref()
            .and_then(|path| config_state.lock().unwrap().source_for(path)),
    ));
    if let Some(archive) = current_archive.borrow().as_ref() {
        set_archive_summary(&ui, archive);
        let source = current_source.lock().unwrap().clone();
        set_source_state(&ui, source.as_ref());
    } else {
        ui.set_source_status("Aucune source de courrier configurée".into());
        ui.set_archive_message_count("Aucune archive ouverte".into());
    }

    let weak: Weak<MailWindow> = ui.as_weak();
    let search_index = current_index.clone();
    ui.on_search_changed(move |query| {
        if let Some(ui) = weak.upgrade() {
            if let Some(index) = search_index.borrow().as_ref() {
                load_results(&ui, index, query.as_str());
            }
        }
    });

    let weak = ui.as_weak();
    let search_index = current_index.clone();
    ui.on_search_submitted(move |query| {
        if let Some(ui) = weak.upgrade() {
            if let Some(index) = search_index.borrow().as_ref() {
                load_results(&ui, index, query.as_str());
            }
        }
    });

    let weak = ui.as_weak();
    let clear_search_index = current_index.clone();
    ui.on_clear_search(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_query(SharedString::default());
            if let Some(index) = clear_search_index.borrow().as_ref() {
                load_results(&ui, index, "");
            }
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
        ui.set_source_status("Synchronisation Gmail en cours…".into());
        ui.set_sync_progress("Préparation de la synchronisation…".into());
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
                                ui.set_source_status("Synchronisation Gmail en cours…".into());
                                ui.set_sync_progress(
                                    format!(
                                        "{} messages examinés · {} nouveaux · {} ajoutés",
                                        snapshot.examined,
                                        snapshot.new_messages,
                                        human_bytes(snapshot.archive_bytes_added)
                                    )
                                    .into(),
                                );
                            }
                        });
                    },
                )
                .map_err(|error| friendly_gmail_error(&error))?;
                let indexing_weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = indexing_weak.upgrade() {
                        ui.set_source_status("Mise à jour de l’index de recherche…".into());
                        ui.set_sync_progress(
                            "Les messages archivés sont maintenant indexés".into(),
                        );
                    }
                });
                index_gmail_archive(&archive)
                    .map_err(|_| "Mise à jour de l’index de recherche échouée".to_string())?;
                Ok(stats)
            })();
            let _ = slint::invoke_from_event_loop(move || {
                running.store(false, Ordering::Release);
                if let Some(ui) = weak.upgrade() {
                    ui.set_syncing(false);
                    match result {
                        Ok(stats) => {
                            let index_ok = ui.invoke_refresh_search_index();
                            set_archive_summary(&ui, &archive);
                            if index_ok {
                                ui.set_source_status(
                                    "Archive à jour · index de recherche à jour".into(),
                                );
                                ui.set_sync_progress(if stats.new_messages == 0 {
                                    "Aucun nouveau message".into()
                                } else {
                                    format!(
                                        "Synchronisation terminée · {} nouveaux · {} ajoutés",
                                        stats.new_messages,
                                        human_bytes(stats.archive_bytes_added)
                                    )
                                    .into()
                                });
                            } else {
                                ui.set_source_status(
                                    "Archive mise à jour · rechargement de l’index échoué".into(),
                                );
                                ui.set_sync_progress(
                                    "Les RAW sont conservés ; l’index doit être reconstruit".into(),
                                );
                            }
                        }
                        Err(error) => {
                            set_archive_summary(&ui, &archive);
                            ui.set_source_status(error.into());
                            ui.set_sync_progress("La recherche locale reste disponible".into());
                        }
                    }
                }
            });
        });
    });

    let selection_generation = Arc::new(AtomicU64::new(0));
    let weak = ui.as_weak();
    let archive_for_selection = current_archive.clone();
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
                    Ok((parsed, raw.len()))
                });
            let _ = slint::invoke_from_event_loop(move || {
                if generation_counter.load(Ordering::Relaxed) != generation {
                    return;
                }
                if let Some(ui) = weak.upgrade() {
                    ui.set_loading(false);
                    match result {
                        Ok((message, raw_bytes)) => {
                            ui.set_message_date(row.date.clone());
                            ui.set_message_from(format!("From: {}", message.sender).into());
                            ui.set_message_to(format!("To: {}", message.recipients).into());
                            ui.set_message_subject(message.subject.into());
                            ui.set_message_body(message.body.into());
                            ui.set_message_attachments(if message.attachment_count > 0 {
                                format!(
                                    "Pièces jointes : {} · RAW : {} octets",
                                    message.attachment_count, raw_bytes
                                )
                                .into()
                            } else {
                                format!("RAW : {} octets", raw_bytes).into()
                            });
                            ui.set_status(
                                format!("Message affiché en {} µs", started.elapsed().as_micros())
                                    .into(),
                            );
                        }
                        Err(error) => {
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
}

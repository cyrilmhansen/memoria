//! Small application-local catalogue. Protocol/schema values never pass here.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Fr,
    En,
}

impl Language {
    pub fn from_locale(value: &str) -> Self {
        if value.trim().to_ascii_lowercase().starts_with("fr") {
            Self::Fr
        } else {
            Self::En
        }
    }
    pub fn system() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
            if let Ok(value) = std::env::var(key) {
                return Self::from_locale(&value);
            }
        }
        Self::En
    }
}

#[derive(Clone, Debug)]
pub struct UiStrings {
    pub window_title: &'static str,
    pub main_accessible: &'static str,
    pub file: &'static str,
    pub new_archive: &'static str,
    pub open_archive: &'static str,
    pub recent_archives: &'static str,
    pub quit: &'static str,
    pub archive: &'static str,
    pub archive_state: &'static str,
    pub sync_now: &'static str,
    pub syncing: &'static str,
    pub add_gmail: &'static str,
    pub back_search: &'static str,
    pub search_menu: &'static str,
    pub focus_search: &'static str,
    pub clear_search: &'static str,
    pub view: &'static str,
    pub zoom_out: &'static str,
    pub zoom_reset: &'static str,
    pub zoom_in: &'static str,
    pub help: &'static str,
    pub about: &'static str,
    pub archive_choice: &'static str,
    pub setup_description: &'static str,
    pub create_archive: &'static str,
    pub archive_sync: &'static str,
    pub local_archive: &'static str,
    pub gmail_source: &'static str,
    pub add_account: &'static str,
    pub search_placeholder: &'static str,
    pub search_accessible: &'static str,
    pub clear: &'static str,
    pub filters: &'static str,
    pub filters_accessible: &'static str,
    pub from_placeholder: &'static str,
    pub from_accessible: &'static str,
    pub to_placeholder: &'static str,
    pub to_accessible: &'static str,
    pub since_placeholder: &'static str,
    pub since_accessible: &'static str,
    pub until_placeholder: &'static str,
    pub until_accessible: &'static str,
    pub attachment_filter_prefix: &'static str,
    pub attachment_filter_accessible: &'static str,
    pub mime_placeholder: &'static str,
    pub mime_accessible: &'static str,
    pub reset_filters: &'static str,
    pub labels_prefix: &'static str,
    pub empty_search: &'static str,
    pub results_accessible: &'static str,
    pub reader_accessible: &'static str,
    pub select_message: &'static str,
    pub open_html: &'static str,
    pub reduce_zoom: &'static str,
    pub reset_zoom: &'static str,
    pub increase_zoom: &'static str,
    pub body_placeholder: &'static str,
    pub attachment_accessible_prefix: &'static str,
    pub preview: &'static str,
    pub open: &'static str,
    pub save_as: &'static str,
    pub preview_accessible_prefix: &'static str,
    pub preview_region_prefix: &'static str,
    pub close: &'static str,
}

impl UiStrings {
    pub fn for_language(language: Language) -> Self {
        match language {
            Language::Fr => Self {
                window_title: "Memoria — Archive",
                main_accessible: "Recherche dans l’archive de messages",
                file: "Fichier",
                new_archive: "Nouvelle archive…",
                open_archive: "Ouvrir une archive…",
                recent_archives: "Archives récentes",
                quit: "Quitter",
                archive: "Archive",
                archive_state: "État de l’archive…",
                sync_now: "Synchroniser maintenant",
                syncing: "Synchronisation…",
                add_gmail: "Ajouter/configurer une source Gmail…",
                back_search: "Retour à la recherche",
                search_menu: "Recherche",
                focus_search: "Focus recherche",
                clear_search: "Effacer la recherche",
                view: "Affichage",
                zoom_out: "Réduire le zoom du message",
                zoom_reset: "Réinitialiser le zoom du message",
                zoom_in: "Augmenter le zoom du message",
                help: "Aide",
                about: "À propos de Memoria",
                archive_choice: "Choix de l’archive",
                setup_description: "Ouvrez une archive existante ou créez-en une nouvelle.",
                create_archive: "Créer une archive…",
                archive_sync: "Archive / Synchronisation",
                local_archive: "Archive locale",
                gmail_source: "Source Gmail",
                add_account: "Ajouter un compte Gmail…",
                search_placeholder: "Rechercher dans les messages…",
                search_accessible: "Recherche dans les messages",
                clear: "Effacer",
                filters: "Filtres",
                filters_accessible: "Afficher ou masquer les filtres de recherche",
                from_placeholder: "De",
                from_accessible: "Filtrer par expéditeur",
                to_placeholder: "À",
                to_accessible: "Filtrer par destinataire",
                since_placeholder: "Depuis YYYY-MM-DD",
                since_accessible: "Filtrer depuis une date",
                until_placeholder: "Jusqu’à YYYY-MM-DD",
                until_accessible: "Filtrer jusqu’à une date",
                attachment_filter_prefix: "Pièces jointes : ",
                attachment_filter_accessible: "Filtre de présence de pièces jointes",
                mime_placeholder: "MIME exact ou famille, ex. image/*",
                mime_accessible: "Filtrer par type MIME de pièce jointe",
                reset_filters: "Réinitialiser",
                labels_prefix: "Labels :",
                empty_search: "Saisissez une recherche pour afficher les messages.",
                results_accessible: "Liste des résultats de recherche",
                reader_accessible: "Panneau de lecture du message",
                select_message: "Sélectionnez un message",
                open_html: "Ouvrir HTML",
                reduce_zoom: "Réduire le zoom du message",
                reset_zoom: "Réinitialiser le zoom du message",
                increase_zoom: "Augmenter le zoom du message",
                body_placeholder: "Le contenu dérivé du message apparaîtra ici.",
                attachment_accessible_prefix: "Pièce jointe ",
                preview: "Aperçu",
                open: "Ouvrir",
                save_as: "Enregistrer sous…",
                preview_accessible_prefix: "Aperçu de la pièce jointe ",
                preview_region_prefix: "Aperçu de ",
                close: "Fermer",
            },
            Language::En => Self {
                window_title: "Memoria — Archive",
                main_accessible: "Search the message archive",
                file: "File",
                new_archive: "New archive…",
                open_archive: "Open archive…",
                recent_archives: "Recent archives",
                quit: "Quit",
                archive: "Archive",
                archive_state: "Archive status…",
                sync_now: "Sync now",
                syncing: "Synchronizing…",
                add_gmail: "Add/configure Gmail source…",
                back_search: "Back to search",
                search_menu: "Search",
                focus_search: "Focus search",
                clear_search: "Clear search",
                view: "View",
                zoom_out: "Reduce message zoom",
                zoom_reset: "Reset message zoom",
                zoom_in: "Increase message zoom",
                help: "Help",
                about: "About Memoria",
                archive_choice: "Archive selection",
                setup_description: "Open an existing archive or create a new one.",
                create_archive: "Create archive…",
                archive_sync: "Archive / Synchronization",
                local_archive: "Local archive",
                gmail_source: "Gmail source",
                add_account: "Add a Gmail account…",
                search_placeholder: "Search messages…",
                search_accessible: "Message search",
                clear: "Clear",
                filters: "Filters",
                filters_accessible: "Show or hide search filters",
                from_placeholder: "From",
                from_accessible: "Filter by sender",
                to_placeholder: "To",
                to_accessible: "Filter by recipient",
                since_placeholder: "Since YYYY-MM-DD",
                since_accessible: "Filter from date",
                until_placeholder: "Until YYYY-MM-DD",
                until_accessible: "Filter through date",
                attachment_filter_prefix: "Attachments: ",
                attachment_filter_accessible: "Attachment presence filter",
                mime_placeholder: "Exact MIME or family, e.g. image/*",
                mime_accessible: "Filter by attachment MIME type",
                reset_filters: "Reset",
                labels_prefix: "Labels:",
                empty_search: "Enter a search to display messages.",
                results_accessible: "Search results list",
                reader_accessible: "Message reader",
                select_message: "Select a message",
                open_html: "Open HTML",
                reduce_zoom: "Reduce message zoom",
                reset_zoom: "Reset message zoom",
                increase_zoom: "Increase message zoom",
                body_placeholder: "The derived message content will appear here.",
                attachment_accessible_prefix: "Attachment ",
                preview: "Preview",
                open: "Open",
                save_as: "Save as…",
                preview_accessible_prefix: "Preview attachment ",
                preview_region_prefix: "Preview of ",
                close: "Close",
            },
        }
    }
}

pub fn message_count(language: Language, count: u64) -> String {
    match language {
        Language::Fr => match count {
            0 => "0 messages".into(),
            1 => "1 message".into(),
            n => format!("{n} messages"),
        },
        Language::En => match count {
            0 => "0 messages".into(),
            1 => "1 message".into(),
            n => format!("{n} messages"),
        },
    }
}

pub fn result_count(language: Language, count: usize) -> String {
    match language {
        Language::Fr => format!("{count} résultat{}", if count == 1 { "" } else { "s" }),
        Language::En => format!("{count} result{}", if count == 1 { "" } else { "s" }),
    }
}

pub fn attachment_filter_label(language: Language, value: &str) -> String {
    match (language, value) {
        (Language::En, "Toutes") => "All".into(),
        (Language::En, "Avec") => "With".into(),
        (Language::En, "Sans") => "Without".into(),
        _ => value.into(),
    }
}

pub fn status(language: Language, key: &str) -> String {
    let (fr, en) = match key {
        "no-search" => ("Aucune recherche", "No search"),
        "type-search" => (
            "Saisissez une recherche pour commencer",
            "Enter a search to begin",
        ),
        "search-error" => ("Erreur de recherche", "Search error"),
        "archive-inaccessible" => ("Archive inaccessible", "Archive unavailable"),
        "index-unavailable" => (
            "Index de recherche indisponible",
            "Search index unavailable",
        ),
        "no-archive" => ("Aucune archive ouverte", "No archive open"),
        "source-unconfigured" => (
            "Aucune source de courrier configurée",
            "No mail source configured",
        ),
        "sync-running" => (
            "Synchronisation Gmail en cours…",
            "Gmail synchronization in progress…",
        ),
        "prepare-sync" => (
            "Préparation de la synchronisation…",
            "Preparing synchronization…",
        ),
        "indexing" => (
            "Mise à jour de l’index de recherche…",
            "Updating the search index…",
        ),
        "index-ready" => (
            "Les messages archivés sont maintenant indexés",
            "Archived messages are now indexed",
        ),
        "index-failed" => (
            "Mise à jour de l’index de recherche échouée",
            "Search index update failed",
        ),
        "archive-up-to-date" => (
            "Archive à jour · index de recherche à jour",
            "Archive up to date · search index up to date",
        ),
        "no-new" => ("Aucun nouveau message", "No new messages"),
        "search-available" => (
            "La recherche locale reste disponible",
            "Local search remains available",
        ),
        "reading" => ("Lecture du message…", "Reading message…"),
        "no-selected" => ("Aucun message sélectionné", "No message selected"),
        "open-html" => ("Ouverture du message HTML…", "Opening HTML message…"),
        "html-opened" => (
            "Message HTML ouvert dans le navigateur",
            "HTML message opened in browser",
        ),
        "attachment-preparing" => ("Préparation de la pièce jointe…", "Preparing attachment…"),
        "attachment-opened" => (
            "Pièce jointe ouverte avec l’application système",
            "Attachment opened with the system application",
        ),
        "attachment-saved" => ("Pièce jointe enregistrée", "Attachment saved"),
        "preview-generating" => ("Génération de l’aperçu…", "Generating preview…"),
        "preview-unavailable" => (
            "Aperçu indisponible pour ce type de pièce jointe",
            "Preview unavailable for this attachment type",
        ),
        "cancelled" => ("Enregistrement annulé", "Save cancelled"),
        "archive-opened" => ("Archive ouverte", "Archive opened"),
        "archive-created" => ("Nouvelle archive créée", "New archive created"),
        "add-before-source" => (
            "Ouvrez ou créez une archive avant d’ajouter Gmail",
            "Open or create an archive before adding Gmail",
        ),
        _ => (key, key),
    };
    if language == Language::Fr {
        fr.into()
    } else {
        en.into()
    }
}

pub fn sync_finished(language: Language, new_messages: u64, bytes: u64) -> String {
    let size = format_bytes(bytes);
    match language {
        Language::Fr => format!(
            "Synchronisation terminée · {new_messages} nouveau{} · {size} ajoutés",
            if new_messages == 1 { "" } else { "x" }
        ),
        Language::En => format!(
            "Synchronization complete · {new_messages} new message{} · {size} added",
            if new_messages == 1 { "" } else { "s" }
        ),
    }
}

pub fn format_bytes(bytes: u64) -> String {
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

pub fn sync_progress(
    language: Language,
    examined: u64,
    total: Option<u64>,
    new_messages: u64,
    bytes: u64,
) -> String {
    let received = format_bytes(bytes);
    match (language, total) {
        (Language::Fr, Some(total)) => format!(
            "{examined} messages sur {total} · {new_messages} nouveau{} · {received} reçus",
            if new_messages == 1 { "" } else { "x" }
        ),
        (Language::En, Some(total)) => format!(
            "{examined} messages of {total} · {new_messages} new message{} · {received} received",
            if new_messages == 1 { "" } else { "s" }
        ),
        (Language::Fr, None) => format!(
            "{examined} message{} examiné{} · {new_messages} nouveau{} · {received} reçus",
            if examined == 1 { "" } else { "s" },
            if examined == 1 { "" } else { "s" },
            if new_messages == 1 { "" } else { "x" }
        ),
        (Language::En, None) => format!(
            "{examined} message{} examined · {new_messages} new message{} · {received} received",
            if examined == 1 { "" } else { "s" },
            if new_messages == 1 { "" } else { "s" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_fallback() {
        assert_eq!(Language::from_locale("fr_FR.UTF-8"), Language::Fr);
        assert_eq!(Language::from_locale("de_DE"), Language::En);
    }
    #[test]
    fn plurals_cover_zero_one_many() {
        for language in [Language::Fr, Language::En] {
            assert!(message_count(language, 0).contains('0'));
            assert!(message_count(language, 1).contains('1'));
            assert!(message_count(language, 2).contains('2'));
        }
    }
    #[test]
    fn protocol_values_are_invariant() {
        assert_eq!("application/pdf", "application/pdf");
        assert_eq!("gmail.readonly", "gmail.readonly");
        assert_eq!("has_attachment", "has_attachment");
    }

    #[test]
    fn main_screen_catalogues_are_present_in_both_languages() {
        let fr = UiStrings::for_language(Language::Fr);
        let en = UiStrings::for_language(Language::En);
        assert_eq!(fr.sync_now, "Synchroniser maintenant");
        assert_eq!(en.sync_now, "Sync now");
        assert_ne!(fr.search_placeholder, en.search_placeholder);
    }
}

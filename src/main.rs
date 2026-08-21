use slint::{Model, ModelRc, SharedString, VecModel, Weak};
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

slint::include_modules!();

const ITEM_COUNT: usize = 100_000;

fn preferences_path() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap())
            .join("slint-apps-workspace-demo.preferences")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| std::env::current_dir().unwrap())
            .join("slint-apps-workspace-demo.preferences")
    }
}

fn load_show_details() -> bool {
    fs::read_to_string(preferences_path())
        .ok()
        .and_then(|value| {
            value
                .lines()
                .find_map(|line| line.strip_prefix("show_details="))
                .map(|value| value != "false")
        })
        .unwrap_or(true)
}

fn load_selected_page() -> SharedString {
    fs::read_to_string(preferences_path())
        .ok()
        .and_then(|value| {
            value
                .lines()
                .find_map(|line| line.strip_prefix("selected_page="))
                .map(SharedString::from)
        })
        .unwrap_or_else(|| "Explorer".into())
}

fn save_show_details(value: bool) {
    let path = preferences_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let page = existing
        .lines()
        .find_map(|line| line.strip_prefix("selected_page="))
        .unwrap_or("Explorer");
    let _ = fs::write(
        path,
        format!(
            "selected_page={page}\nshow_details={}\n",
            if value { "true" } else { "false" }
        ),
    );
}

fn save_selected_page(page: &str) {
    let path = preferences_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let show_details = load_show_details();
    let _ = fs::write(
        path,
        format!(
            "selected_page={page}\nshow_details={}\n",
            if show_details { "true" } else { "false" }
        ),
    );
}

fn make_items(query: &str) -> Vec<SharedString> {
    let needle = query.trim().to_ascii_lowercase();
    (0..ITEM_COUNT)
        .filter_map(|index| {
            let value = format!("Élément synthétique #{index:06}");
            (needle.is_empty() || value.to_ascii_lowercase().contains(&needle))
                .then_some(SharedString::from(value))
        })
        .collect()
}

fn set_items(ui: &DemoWindow, items: Vec<SharedString>) {
    ui.set_filtered_count(items.len() as i32);
    ui.set_selected_index(-1);
    ui.set_selected_item("Aucun élément sélectionné".into());
    ui.set_items(ModelRc::new(VecModel::from(items)));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|argument| argument == "--benchmark") {
        return benchmark();
    }

    let _ = slint::set_xdg_app_id("org.example.SlintAppsWorkspaceDemo");
    let ui = DemoWindow::new()?;
    let initial_items = make_items("");
    set_items(&ui, initial_items);
    ui.set_show_details(load_show_details());
    ui.set_selected_page(load_selected_page());

    let weak: Weak<DemoWindow> = ui.as_weak();
    ui.on_navigate(move |page| {
        save_selected_page(page.as_str());
        if let Some(ui) = weak.upgrade() {
            ui.set_selected_page(page);
        }
    });

    let weak = ui.as_weak();
    ui.on_search_changed(move |query| {
        let items = make_items(query.as_str());
        if let Some(ui) = weak.upgrade() {
            set_items(&ui, items);
        }
    });

    let weak = ui.as_weak();
    ui.on_item_selected(move |index| {
        if let Some(ui) = weak.upgrade() {
            ui.set_selected_index(index);
            let model = ui.get_items();
            if let Some(item) = model.row_data(index as usize) {
                ui.set_selected_item(item);
            }
        }
    });

    let weak = ui.as_weak();
    ui.on_open_dialog(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_dialog_open(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_close_dialog(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_dialog_open(false);
        }
    });

    ui.on_preferences_changed(move |value| {
        save_show_details(value);
    });

    let cancel = Arc::new(AtomicBool::new(false));
    let weak = ui.as_weak();
    let cancel_for_start = cancel.clone();
    ui.on_start_task(move || {
        if cancel_for_start.load(Ordering::Relaxed) {
            return;
        }
        cancel_for_start.store(false, Ordering::Relaxed);
        if let Some(ui) = weak.upgrade() {
            ui.set_task_running(true);
            ui.set_task_progress(0);
            ui.set_status("Tâche en cours…".into());
        }
        let worker_weak = weak.clone();
        let worker_cancel = cancel_for_start.clone();
        thread::spawn(move || {
            for progress in 0..=100 {
                if worker_cancel.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
                let update = worker_weak.clone();
                let cancelled = worker_cancel.load(Ordering::Relaxed);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = update.upgrade() {
                        ui.set_task_progress(progress);
                        if progress == 100 {
                            ui.set_task_running(false);
                            ui.set_status("Tâche terminée".into());
                        } else if cancelled {
                            ui.set_task_running(false);
                            ui.set_status("Tâche annulée".into());
                        }
                    }
                });
            }
        });
    });

    let cancel_for_cancel = cancel.clone();
    ui.on_cancel_task(move || {
        cancel_for_cancel.store(true, Ordering::Relaxed);
    });

    ui.run()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn resident_memory_kb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn resident_memory_kb() -> u64 {
    0
}

fn benchmark() -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let empty_items: Vec<SharedString> = Vec::new();
    let empty_memory = resident_memory_kb();
    let list_started = Instant::now();
    let populated_items = make_items("");
    let populated_memory = resident_memory_kb();
    assert_eq!(empty_items.len(), 0);
    assert_eq!(populated_items.len(), ITEM_COUNT);
    println!("startup_to_dataset_ms={}", started.elapsed().as_millis());
    println!("memory_without_items_kb={empty_memory}");
    println!("populate_100000_ms={}", list_started.elapsed().as_millis());
    println!("memory_with_100000_items_kb={populated_memory}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_requested_synthetic_dataset() {
        let items = make_items("");
        assert_eq!(items.len(), ITEM_COUNT);
        assert_eq!(items[0].as_str(), "Élément synthétique #000000");
        assert_eq!(
            items[ITEM_COUNT - 1].as_str(),
            "Élément synthétique #099999"
        );
    }

    #[test]
    fn filters_interactively_without_changing_item_format() {
        let items = make_items("99999");
        assert!(!items.is_empty());
        assert!(items.iter().all(|item| item.contains("99999")));
    }
}

use std::{cell::RefCell, error::Error, path::PathBuf, rc::Rc};

use slint::{ComponentHandle, Timer, TimerMode};
use url::Url;
use wry::{
    dpi::{PhysicalPosition, PhysicalSize, Position, Size},
    Rect, WebViewBuilder,
};

slint::include_modules!();

fn bounds_for(window: &slint::Window) -> Rect {
    let size = window.size();
    let x = size.width / 2;
    Rect {
        position: Position::Physical(PhysicalPosition::new(x as i32, 0)),
        size: Size::Physical(PhysicalSize::new(size.width - x, size.height)),
    }
}

fn file_url(path: PathBuf) -> Result<String, Box<dyn Error>> {
    Ok(Url::from_file_path(path)
        .map_err(|_| "cannot convert local asset path to file URL")?
        .to_string())
}

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    gtk::init()?;

    let ui = MainWindow::new()?;
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    let demo_url = file_url(assets.join("demo.html"))?;
    ui.show()?;
    let webview = Rc::new(RefCell::new(None));
    let init_view = Rc::clone(&webview);
    let init_ui = ui.as_weak();
    Timer::single_shot(std::time::Duration::from_millis(100), move || {
        let Some(ui) = init_ui.upgrade() else { return };
        let window_handle = ui.window().window_handle();
        let result = WebViewBuilder::new()
            .with_url(&demo_url)
            .with_bounds(bounds_for(&ui.window()))
            .with_focused(false)
            .with_javascript_disabled()
            .with_general_autofill_enabled(false)
            .with_navigation_handler(|url| url.starts_with("file://"))
            .build_as_child(&window_handle);
        match result {
            Ok(view) => {
                eprintln!("webview_created=true");
                *init_view.borrow_mut() = Some(view);
            }
            Err(error) => eprintln!("webview_created=false error={error:?}"),
        }
    });
    let resize_view = Rc::clone(&webview);
    let resize_ui = ui.as_weak();
    let timer = Timer::default();
    timer.start(
        TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
            #[cfg(target_os = "linux")]
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            if let (Some(ui), Some(view)) = (resize_ui.upgrade(), resize_view.borrow().as_ref()) {
                let _ = view.set_bounds(bounds_for(&ui.window()));
            }
        },
    );

    let toggle_view = Rc::clone(&webview);
    let visible = Rc::new(RefCell::new(true));
    let toggle_visible = Rc::clone(&visible);
    ui.on_toggle_webview(move || {
        if let Some(view) = toggle_view.borrow().as_ref() {
            let mut visible = toggle_visible.borrow_mut();
            *visible = !*visible;
            let _ = view.set_visible(*visible);
        }
    });

    let change_view = Rc::clone(&webview);
    ui.on_change_document(move || {
        if let Some(view) = change_view.borrow().as_ref() {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/alternate.html");
            if let Ok(url) = file_url(path) {
                let _ = view.load_url(&url);
            }
        }
    });

    ui.run()?;
    drop(timer);
    Ok(())
}

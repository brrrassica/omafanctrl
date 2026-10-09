//! `omafanctrl-gui` — the GTK4 + libadwaita desktop application.
//!
//! An adaptive, Wayland-native front end for the `omafanctrl` daemon. The shell
//! is an [`adw::NavigationSplitView`] (slim sidebar + content) whose primary
//! page is an [`adw::MultiLayoutView`] dashboard that reflows for Hyprland's
//! dwindle tiling sizes. Settings live behind a header menu button. Live data
//! arrives from the daemon's `StateChanged`/`ConfigChanged` signals via a
//! background D-Bus client.

mod chart;
mod client;
mod curve;
mod curve_page;
mod dashboard;
mod sensors_page;
mod settings;
mod state;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use omafanctrl_core::config::Config;

use crate::client::{Command, Update};
use crate::curve_page::CurvePage;
use crate::dashboard::Dashboard;
use crate::sensors_page::SensorsPage;
use crate::settings::SettingsPage;
use crate::state::AppState;

/// The application ID, matching the D-Bus name.
const APP_ID: &str = "org.omarchy.omafanctrl";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// Build the application window.
fn build_ui(app: &adw::Application) {
    let handle = client::spawn();
    let toast_overlay = adw::ToastOverlay::new();
    let state = AppState {
        handle: handle.clone(),
        config: Rc::new(RefCell::new(None)),
        toast_overlay: toast_overlay.clone(),
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("omafanctrl")
        .default_width(920)
        .default_height(660)
        .build();

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);

    let dashboard = Rc::new(Dashboard::new(&state));
    let curve_page = Rc::new(CurvePage::new(&state));
    let sensors_page = Rc::new(SensorsPage::new(&state));
    let settings_page = Rc::new(SettingsPage::new(&state));

    stack.add_named(dashboard.widget(), Some("dashboard"));
    stack.add_named(curve_page.widget(), Some("curve"));
    stack.add_named(sensors_page.widget(), Some("sensors"));

    let status = adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title("Daemon unavailable")
        .description(
            "The omafanctrl daemon is not running. Start omafanctrld and reopen this window.",
        )
        .build();
    stack.add_named(&status, Some("error"));

    // Slim sidebar: the dashboard plus the two secondary pages.
    let sidebar = gtk::ListBox::new();
    sidebar.set_selection_mode(gtk::SelectionMode::Single);
    sidebar.add_css_class("navigation-sidebar");
    let entries = [
        (
            "dashboard",
            "Dashboard",
            "utilities-system-monitor-symbolic",
        ),
        ("curve", "Smart Curve", "office-chart-line-symbolic"),
        ("sensors", "Sensors", "sensors-applet-symbolic"),
    ];
    for (_, title, icon) in entries {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        sidebar.append(&row);
    }

    let content_page = adw::NavigationPage::new(&stack, "Dashboard");
    let sidebar_page = adw::NavigationPage::new(&sidebar, "omafanctrl");

    let split = adw::NavigationSplitView::new();
    split.set_sidebar(Some(&sidebar_page));
    split.set_content(Some(&content_page));

    // Header bar with a clearly iconed and labeled Settings menu button.
    let header = adw::HeaderBar::new();
    let menu = gio::Menu::new();
    menu.append(Some("Preferences…"), Some("win.preferences"));
    menu.append(Some("Reload configuration"), Some("win.reload-config"));
    menu.append(Some("About omafanctrl"), Some("win.about"));
    let settings_button = gtk::MenuButton::builder()
        .icon_name("preferences-system-symbolic")
        .label("Settings")
        .menu_model(&menu)
        .build();
    header.pack_end(&settings_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&split));

    toast_overlay.set_child(Some(&toolbar));
    window.set_content(Some(&toast_overlay));

    // Settings actions.
    let preferences_action = gio::SimpleAction::new("preferences", None);
    let settings_dialog = settings_page.dialog().clone();
    let window_for_prefs = window.clone();
    preferences_action.connect_activate(move |_, _| {
        settings_dialog.present(Some(&window_for_prefs));
    });
    window.add_action(&preferences_action);

    let reload_action = gio::SimpleAction::new("reload-config", None);
    let state_for_reload = state.clone();
    reload_action.connect_activate(move |_, _| {
        state_for_reload.handle.send(Command::ReloadConfig);
        state_for_reload.toast("Reloading configuration…");
    });
    window.add_action(&reload_action);

    let about_action = gio::SimpleAction::new("about", None);
    let window_for_about = window.clone();
    about_action.connect_activate(move |_, _| {
        let about = adw::AboutDialog::builder()
            .application_name("omafanctrl")
            .application_icon(APP_ID)
            .developer_name("omafanctrl contributors")
            .version(env!("CARGO_PKG_VERSION"))
            .website("https://github.com/omafanctrl/omafanctrl")
            .license_type(gtk::License::MitX11)
            .build();
        about.present(Some(&window_for_about));
    });
    window.add_action(&about_action);

    // Sidebar selection drives the content stack.
    let stack_for_select = stack.clone();
    let content_page_for_select = content_page.clone();
    sidebar.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let index = row.index().max(0) as usize;
            if let Some((name, title, _)) = entries.get(index) {
                stack_for_select.set_visible_child_name(name);
                content_page_for_select.set_title(title);
            }
        }
    });
    if let Some(row) = sidebar.row_at_index(0) {
        sidebar.select_row(Some(&row));
    }

    // Drain updates on the GTK main loop.
    let updates = handle.updates.clone();
    let state_for_updates = state.clone();
    let stack_for_updates = stack.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        while let Ok(update) = updates.try_recv() {
            match update {
                Update::State(snapshot) => {
                    dashboard.apply_state(&snapshot);
                    curve_page.apply_state(&snapshot);
                    sensors_page.apply_state(&snapshot);
                    settings_page.apply_state(&snapshot);
                }
                Update::Config(text) => match text.parse::<Config>() {
                    Ok(config) => {
                        *state_for_updates.config.borrow_mut() = Some(config.clone());
                        dashboard.apply_config(&config);
                        curve_page.apply_config(&config);
                        sensors_page.apply_config(&config);
                        settings_page.apply_config(&config);
                    }
                    Err(error) => {
                        state_for_updates.toast(&format!("invalid configuration: {error}"));
                    }
                },
                Update::Error(message) => {
                    stack_for_updates.set_visible_child_name("error");
                    state_for_updates.toast(&message);
                }
            }
        }
        glib::ControlFlow::Continue
    });

    window.present();
}

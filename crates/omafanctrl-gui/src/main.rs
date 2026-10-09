//! `omafanctrl-gui` — the GTK4 + libadwaita desktop application.
//!
//! An adaptive, Wayland-native front end for the `omafanctrl` daemon. The shell
//! is an [`adw::ToolbarView`] with a header bar and an [`adw::ViewSwitcher`] tab
//! strip directly below it, driving an [`adw::ViewStack`]. The primary page is
//! an [`adw::MultiLayoutView`] dashboard that reflows for Hyprland's dwindle
//! tiling sizes. Settings live behind a header menu button. Live data arrives
//! from the daemon's `StateChanged`/`ConfigChanged` signals via a background
//! D-Bus client.

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

/// The width (in `sp`) below which the view switcher shows icons only.
const SWITCHER_NARROW_WIDTH: f64 = 550.0;

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

    let dashboard = Rc::new(Dashboard::new(&state));
    let curve_page = Rc::new(CurvePage::new(&state));
    let sensors_page = Rc::new(SensorsPage::new(&state));
    let settings_page = Rc::new(SettingsPage::new(&state));

    // The tabbed view stack: Dashboard, Smart Curve, Sensors, plus a hidden
    // error page that is never shown in the switcher.
    let stack = adw::ViewStack::new();
    let dashboard_page = stack.add_titled(dashboard.widget(), Some("dashboard"), "Dashboard");
    dashboard_page.set_icon_name(Some("utilities-system-monitor-symbolic"));
    let curve_stack_page = stack.add_titled(curve_page.widget(), Some("curve"), "Smart Curve");
    curve_stack_page.set_icon_name(Some("office-chart-line-symbolic"));
    let sensors_stack_page = stack.add_titled(sensors_page.widget(), Some("sensors"), "Sensors");
    sensors_stack_page.set_icon_name(Some("sensors-applet-symbolic"));

    let status = adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title("Daemon unavailable")
        .description(
            "The omafanctrl daemon is not running. Start omafanctrld and reopen this window.",
        )
        .build();
    let error_page = stack.add_named(&status, Some("error"));
    error_page.set_visible(false);

    // The tab strip, directly below the header bar. It fills the width up to a
    // generous cap and is centered, so it never stretches edge to edge or
    // squeezes the labels.
    let switcher = adw::ViewSwitcher::new();
    switcher.set_stack(Some(&stack));
    switcher.set_policy(adw::ViewSwitcherPolicy::Wide);
    switcher.set_hexpand(true);
    let switcher_clamp = adw::Clamp::new();
    switcher_clamp.set_maximum_size(720);
    switcher_clamp.set_tightening_threshold(400);
    switcher_clamp.set_child(Some(&switcher));

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

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&switcher_clamp);
    content.append(&stack);
    stack.set_vexpand(true);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    toast_overlay.set_child(Some(&toolbar));
    window.set_content(Some(&toast_overlay));

    // Collapse the switcher to icons only at narrow tiling widths.
    add_switcher_breakpoint(
        &window,
        &switcher,
        adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            SWITCHER_NARROW_WIDTH,
            adw::LengthUnit::Sp,
        ),
        adw::ViewSwitcherPolicy::Narrow,
    );
    add_switcher_breakpoint(
        &window,
        &switcher,
        adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MinWidth,
            SWITCHER_NARROW_WIDTH,
            adw::LengthUnit::Sp,
        ),
        adw::ViewSwitcherPolicy::Wide,
    );

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

    // Drain updates on the GTK main loop.
    let updates = handle.updates.clone();
    let state_for_updates = state.clone();
    let stack_for_updates = stack.clone();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        while let Ok(update) = updates.try_recv() {
            match update {
                Update::State(snapshot) => {
                    recover_from_error(&stack_for_updates);
                    dashboard.apply_state(&snapshot);
                    curve_page.apply_state(&snapshot);
                    sensors_page.apply_state(&snapshot);
                    settings_page.apply_state(&snapshot);
                }
                Update::Config(text) => match text.parse::<Config>() {
                    Ok(config) => {
                        recover_from_error(&stack_for_updates);
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

/// Return to the dashboard once the daemon is reachable again.
fn recover_from_error(stack: &adw::ViewStack) {
    if stack.visible_child_name().as_deref() == Some("error") {
        stack.set_visible_child_name("dashboard");
    }
}

/// Attach a breakpoint that switches the view switcher's policy.
fn add_switcher_breakpoint(
    window: &adw::ApplicationWindow,
    switcher: &adw::ViewSwitcher,
    condition: adw::BreakpointCondition,
    policy: adw::ViewSwitcherPolicy,
) {
    let breakpoint = adw::Breakpoint::new(condition);
    breakpoint.add_setter(switcher, "policy", Some(&policy.to_value()));
    window.add_breakpoint(breakpoint);
}

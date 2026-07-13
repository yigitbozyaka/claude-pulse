mod data;
mod icon;

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};

use data::{Account, AccountData};

struct AppState {
    accounts: Vec<Account>,
    active: usize,
}
type Shared = Mutex<AppState>;

struct Ui {
    checks: Vec<CheckMenuItem<Wry>>,
}

// ─── Commands (invoked from the frontend) ────────────────────────────

#[tauri::command]
fn list_accounts(state: State<Shared>) -> Vec<String> {
    state
        .lock()
        .unwrap()
        .accounts
        .iter()
        .map(|a| a.name.clone())
        .collect()
}

#[tauri::command]
fn get_active(state: State<Shared>) -> usize {
    state.lock().unwrap().active
}

#[tauri::command]
fn get_account(idx: usize, state: State<Shared>) -> Option<AccountData> {
    let mut st = state.lock().unwrap();
    st.accounts.get_mut(idx).map(|a| a.get_data())
}

#[tauri::command]
fn set_active(app: AppHandle, idx: usize, state: State<Shared>) -> Option<AccountData> {
    let data = {
        let mut st = state.lock().unwrap();
        if idx >= st.accounts.len() {
            return None;
        }
        st.active = idx;
        st.accounts[idx].get_data()
    };
    update_tray(&app, &data);
    update_checks(&app, idx);
    Some(data)
}

// ─── Tray helpers ────────────────────────────────────────────────────

fn tooltip(data: &AccountData) -> String {
    match &data.usage {
        None => format!("{} - loading...", data.name),
        Some(u) => {
            let mut parts = vec![
                format!("Session: {:.0}%", u.session.utilization),
                format!("Weekly: {:.0}%", u.weekly_all.utilization),
            ];
            if u.subscription.has_sonnet && u.weekly_sonnet.resets_at.is_some() {
                parts.push(format!("Sonnet: {:.0}%", u.weekly_sonnet.utilization));
            }
            format!(
                "{} · Claude {}\n{}\nSession resets in {}  |  Weekly resets in {}\nUpdated {}",
                data.name,
                u.subscription.display,
                parts.join("  |  "),
                u.session.resets_in,
                u.weekly_all.resets_in,
                u.updated_ago
            )
        }
    }
}

fn update_tray(app: &AppHandle, data: &AccountData) {
    if let Some(tray) = app.tray_by_id("main") {
        let (rgba, w, h) = icon::battery_icon(data.session_pct);
        let img = tauri::image::Image::new_owned(rgba, w, h);
        let _ = tray.set_icon(Some(img));
        let _ = tray.set_tooltip(Some(tooltip(data)));
    }
}

fn update_checks(app: &AppHandle, idx: usize) {
    if let Some(ui) = app.try_state::<Mutex<Ui>>() {
        if let Ok(ui) = ui.lock() {
            for (i, c) in ui.checks.iter().enumerate() {
                let _ = c.set_checked(i == idx);
            }
        }
    }
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn select_from_tray(app: &AppHandle, idx: usize) {
    let data = {
        let state = app.state::<Shared>();
        let mut st = state.lock().unwrap();
        if idx >= st.accounts.len() {
            return;
        }
        st.active = idx;
        st.accounts[idx].get_data()
    };
    update_tray(app, &data);
    update_checks(app, idx);
    let _ = app.emit("account-changed", idx);
}

fn refresh_active(app: &AppHandle) {
    let data = {
        let state = app.state::<Shared>();
        let mut st = state.lock().unwrap();
        let a = st.active;
        st.accounts[a].get_data()
    };
    update_tray(app, &data);
    let _ = app.emit("usage-updated", ());
}

// ─── Entry point ─────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cache_dir = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ClaudeUsage");
    let _ = std::fs::create_dir_all(&cache_dir);
    let accounts = data::discover_accounts(&cache_dir);

    tauri::Builder::default()
        .manage(Mutex::new(AppState { accounts, active: 0 }))
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            get_active,
            get_account,
            set_active
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            let names: Vec<String> = {
                let st = handle.state::<Shared>();
                let st = st.lock().unwrap();
                st.accounts.iter().map(|a| a.name.clone()).collect()
            };

            // ── tray menu ──
            let open = MenuItemBuilder::with_id("open", "Open Dashboard").build(app)?;
            let refresh = MenuItemBuilder::with_id("refresh", "Refresh").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let mut checks: Vec<CheckMenuItem<Wry>> = Vec::new();
            let mut mb = MenuBuilder::new(app).item(&open);
            if names.len() > 1 {
                mb = mb.separator();
                for (i, name) in names.iter().enumerate() {
                    let c = CheckMenuItemBuilder::with_id(format!("acct:{i}"), name)
                        .checked(i == 0)
                        .build(app)?;
                    mb = mb.item(&c);
                    checks.push(c);
                }
            }
            let menu = mb
                .separator()
                .item(&refresh)
                .separator()
                .item(&quit)
                .build()?;
            app.manage(Mutex::new(Ui { checks }));

            // ── tray icon (neutral until first refresh lands) ──
            let (rgba, w, h) = icon::battery_icon(0.0);
            let img = tauri::image::Image::new_owned(rgba, w, h);
            let _tray = TrayIconBuilder::with_id("main")
                .icon(img)
                .tooltip("Claude Usage - loading...")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_window(app),
                    "refresh" => refresh_active(app),
                    "quit" => app.exit(0),
                    other if other.starts_with("acct:") => {
                        if let Ok(idx) = other[5..].parse::<usize>() {
                            select_from_tray(app, idx);
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // ── hide to tray instead of quitting on window close ──
            if let Some(win) = app.get_webview_window("main") {
                let w2 = win.clone();
                win.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w2.hide();
                    }
                });
            }

            // ── background refresh: immediate first fetch, then every 60s ──
            std::thread::spawn(move || {
                refresh_active(&handle);
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                    refresh_active(&handle);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

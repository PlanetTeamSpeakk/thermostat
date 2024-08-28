#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")] // Hide console window on Windows if we're not debugging.
#![allow(non_snake_case)] // The project name is also the name of the process, which should have a capital T.

use slint::{private_unstable_api::re_exports::{EventResult, KeyEvent}, WindowPosition, PhysicalPosition, Weak};
use tokio::{task::JoinHandle, time::{sleep, Instant}};
use std::{fs, path::{Path, PathBuf}, io::{BufWriter, Write}, time::Duration};
use directories::ProjectDirs;
use anyhow::Result;
use log::{error, info};

slint::include_modules!();

#[cfg(not(debug_assertions))]
const API_URL: &str = "http://192.168.178.48:5567/";
#[cfg(debug_assertions)]
const API_URL: &str = "http://192.168.178.48:5568/";
const OPTIONS_FILE: &str = "options.json";

const WINDOW_OPACITY_FOCUSED: f32 = 0.9;
const WINDOW_OPACITY_UNFOCUSED: f32 = 0.35;

const TEMPERATURE_STEP: f32 = 0.5;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // Get data dir, if possible.
    // Default to current directory.
    let app_dir = ProjectDirs::from("com", "PTSMods", "Thermostat");
    let mut data_dir = app_dir.map_or_else(|| std::env::current_dir().unwrap().to_owned(), |pds| pds.data_dir().to_owned());
    
    if let Err(e) = fs::create_dir_all(&data_dir) {
        error!("Could not create data dir: {:?}", e);
        data_dir = std::env::current_dir().unwrap(); // Fallback to current directory.
    }
    info!("Data dir: {:?}", data_dir);

    let options_path = data_dir.join(OPTIONS_FILE);

    // Read options from disk.
    let options = 
        if Path::new(&options_path).exists() { Some(fs::read_to_string(&options_path)?) }
        else { None };
    let options = options
        .map(|s| serde_json::from_str(&s))
        .unwrap_or(Ok(Options::default()));

    if let Err(err) = &options {
        error!("Could not read options from disk: {:?}", err);
    }
    let options = options.unwrap();

    // Run the UI.
    let ui = AppWindow::new()?;
    ui.set_is_preview(false); // Disable preview mode.
    ui.global::<Singletons>().set_options(options.app_options.clone());
    run_ui(ui, options, &options_path).await
}

/// Registers event handlers and runs the UI.
async fn run_ui(ui: AppWindow, mut options: Options, options_path: &PathBuf) -> Result<()> {
    // Acquire the config and state from the API asynchronously.
    let ui_handle = ui.as_weak();
    tokio::spawn(async move {
        let resp = get_api_async(true).await;

        let _ = ui_handle.upgrade_in_event_loop(move |ui| {
            if let Ok(resp) = resp {
                if !resp.success {
                    error!("API returned an error: {}", resp.error.unwrap());
                    return;
                }
                
                let singletons = ui.global::<Singletons>();
                let data = resp.data.unwrap();
                singletons.set_config(data.config.unwrap().into());
                singletons.set_state(data.state.into());

                // Hide the splash window.
                ui.invoke_hide_splash();
            }
        });
    });

    // Register event handlers
    register_target_temp_handler(&ui);
    register_window_move_handler(&ui);
    register_quit_handler(&ui);
    register_key_handler(&ui);
    register_focus_handler(&ui);

    start_ui_updater(&ui);

    // Restore previous window position
    ui.window().set_position(WindowPosition::Physical(options.window_pos));
    ui.run()?;
    
    // Save options upon shutdown.
    options.window_pos = ui.window().position();
    options.app_options = ui.global::<Singletons>().get_options();
    save_options(&options, options_path)?;

    Ok(())
}

fn register_target_temp_handler(ui: &AppWindow) {
    let ui_handle = ui.as_weak();
    let mut task: Option<JoinHandle<()>> = None;
    let mut last: Instant = Instant::now();
    const UPDATE_MARGIN: Duration = Duration::from_millis(250);

    ui.on_request_config_change(move || {
        let ui_handle = ui_handle.clone();

        // If there is already a task running, cancel it.
        if let Some(jh) = &task {
            if !jh.is_finished() {
                jh.abort();
            }
        }

        // If the last update was less than 250ms ago, schedule an update for later.
        // Otherwise, update immediately.
        let do_delay = last.elapsed() < UPDATE_MARGIN;
        last = Instant::now();

        // Spawn a new task that will update the config after 250ms.
        let jh = tokio::spawn(async move {
            if do_delay {
                tokio::time::sleep(UPDATE_MARGIN).await; // Wait for the user to stop modifying.
            }

            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                let cfg = ui.global::<Singletons>().get_config().into();
                update_config(&ui, cfg);
            });
        });

        if do_delay {
            task = Some(jh);
        }
    });
}

fn register_window_move_handler(ui: &AppWindow) {
    let ui_handle = ui.as_weak();
    ui.on_request_window_move(move |dx: i32, dy: i32| {
        let ui = ui_handle.unwrap();
        let pos = ui.window().position(); // Current position

        // Move the window along with the cursor.
        ui.window().set_position(WindowPosition::Physical(PhysicalPosition { x: pos.x + dx, y: pos.y + dy }));
    });
}

fn register_quit_handler(ui: &AppWindow) {
    let ui_handle = ui.as_weak();
    ui.on_request_quit(move || {
        let ui = ui_handle.unwrap();
        let _ = ui.window().hide(); // We do not care about the result here.
    });
}

fn register_key_handler(ui: &AppWindow) {
    let ui_handle = ui.as_weak();
    ui.on_key_pressed(move |e: KeyEvent| {
        let ui = ui_handle.unwrap();
        match e.text.as_str() {
            "\u{1b}" => { // Escape key
                let _ = ui.window().hide(); // We do not care about the result here.
                EventResult::Accept
            },
            "f" => {
                modify_config(&ui, |cfg: &mut ThermostatConfig| {
                    cfg.force = !cfg.force;
                });
                EventResult::Accept
            },
            "\u{f700}" => { // Up arrow
                modify_config(&ui, |cfg: &mut ThermostatConfig| {
                    cfg.target_temp += TEMPERATURE_STEP;
                });
                EventResult::Accept
            },
            "\u{f701}" => { // Down arrow
                modify_config(&ui, |cfg: &mut ThermostatConfig| {
                    cfg.target_temp -= TEMPERATURE_STEP;
                });
                EventResult::Accept
            },
            _ => EventResult::Reject
        }
    });
}

fn register_focus_handler(ui: &AppWindow) {
    let ui_handle = ui.as_weak();
    ui.on_focus_change(move |has_focus| {
        let ui_handle = ui_handle.clone();

        tokio::spawn(async move {
            // is_co2_focused is not yet updated at this point, but it is directly after.
            // Hence, we wait just a tiny moment before checking it.
            sleep(Duration::from_micros(5)).await;

            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                if has_focus || ui.get_is_co2_focused() {
                    ui.set_window_opacity(WINDOW_OPACITY_FOCUSED);
                } else {
                    ui.set_window_opacity(WINDOW_OPACITY_UNFOCUSED);
                }
            });
        });
    });
}

fn start_ui_updater(ui: &AppWindow) {
    // Periodically update the UI with the latest data from the API.
    let ui_handle = ui.as_weak();
    tokio::spawn(async move {
        const UPDATE_INTERVAL: Duration = Duration::from_secs(15);
        let mut interval = tokio::time::interval_at(Instant::now() + UPDATE_INTERVAL, UPDATE_INTERVAL);

        loop {
            interval.tick().await; // Run every 15 seconds

            match get_api_async(false).await {
                Ok(resp) => try_apply_response(ui_handle.clone(), resp),
                Err(err) => error!("Could not get metrics from API: {:?}", err),
            }
        }
    });
}

/// Writes the options to disk in JSON format.
fn save_options(options: &Options, path: &PathBuf) -> Result<()> {
    let mut writer = BufWriter::new(fs::File::create(path)?);
    serde_json::to_writer_pretty(&mut writer, options)?;
    writer.flush()?;
    Ok(())
}

/// Modify the thermostat config.
fn modify_config(ui: &AppWindow, f: impl FnOnce(&mut ThermostatConfig)) {
    let singletons = ui.global::<Singletons>(); // Get the Singletons module.

    let mut cfg: ThermostatConfig = singletons.get_config().into(); // Get config.
    f(&mut cfg); // Modify config.
    singletons.set_config(cfg.into()); // Set config.

    update_config(ui, cfg);
}

// Sends a PATCH request to the API to update the config.
// This is done asynchronously.
fn update_config(ui: &AppWindow, cfg: ThermostatConfig) {
    let ui_handle = ui.as_weak();
    tokio::spawn(async move {
        // Send PATCH request to API
        let res = patch_api_async(&reqwest::Client::new(), cfg).await;

        if let Ok(resp) = res {
            try_apply_response(ui_handle, resp);
        } else {
            error!("Error sending API request: {:?}", res.err());
        }
    });
}

fn try_apply_response(ui_handle: Weak<AppWindow>, resp: APIResponse) {
    if resp.success {
        // Ignore result, we don't care if it actually updated.
        // If it didn't, the UI is probably gone anyway.
        let _ = ui_handle.upgrade_in_event_loop(move |ui| {
            ui.global::<Singletons>().set_state(resp.data.unwrap().state.into());
        });
    } else {
        error!("API returned an error: {}", resp.error.unwrap());
    }
}

/// Send a PATCH request to the API.
async fn patch_api_async(client: &reqwest::Client, new_config: ThermostatConfig) -> Result<APIResponse, reqwest::Error> {
    info!("Updating config to {:?}", new_config);

    client.patch(API_URL)
        .json(&new_config)
        .send()
        .await?
        .json::<APIResponse>()
        .await
}

/// Get the current thermostat config and states from the API.
async fn get_api_async(include_config: bool) -> Result<APIResponse, reqwest::Error> {
    reqwest::get(API_URL.to_owned() + "?include_config=" + &include_config.to_string())
        .await?
        .json()
        .await
}

// Thermostat config
#[derive(serde::Deserialize, serde::Serialize)]
#[derive(Debug, Clone, Copy)]
struct ThermostatConfig {
    master_switch: bool,
    force: bool,
    target_temp: f32,
    co2_target: Option<i32>,
}

// Allow for conversion between the slint-generated Config struct and the ThermostatConfig struct.
impl From<Config> for ThermostatConfig {
    fn from(cfg: Config) -> Self {
        Self {
            master_switch: cfg.master_switch,
            force: cfg.force,
            target_temp: cfg.target_temp,
            co2_target: if cfg.require_co2 {Some(cfg.co2_target)} else {None},
        }
    }
}

impl From<ThermostatConfig> for Config {
    fn from(cfg: ThermostatConfig) -> Self {
        Self {
            master_switch: cfg.master_switch,
            force: cfg.force,
            target_temp: cfg.target_temp,
            require_co2: cfg.co2_target.is_some(),
            co2_target: cfg.co2_target.unwrap_or(500),
        }
    }
}

#[derive(serde::Deserialize, Debug)]
struct APIResponse {
    success: bool,
    data: Option<APIResponseData>,
    error: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
struct APIResponseData {
    config: Option<ThermostatConfig>,
    state: APIResponseStateData,
}

#[derive(serde::Deserialize, Debug)]
struct APIResponseStateData {
    available: bool,
    temperature: f32,
    co2: i32,
    is_heating: bool,
}

impl From<APIResponseStateData> for State {
    fn from(state: APIResponseStateData) -> Self {
        Self {
            available: state.available,
            current_temp: state.temperature,
            co2: state.co2,
            is_heating: state.is_heating,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct Options {
    #[serde(with = "PhysicalPositionRemote")]
    window_pos: PhysicalPosition,
    #[serde(with = "AppOptionsRemote")]
    app_options: AppOptions,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            window_pos: PhysicalPosition { x: 190, y: 190 },
            app_options: AppOptions::default(),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(remote = "PhysicalPosition")]
struct PhysicalPositionRemote {
    x: i32,
    y: i32,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(remote = "AppOptions")]
struct AppOptionsRemote {
    on_top: bool,
}

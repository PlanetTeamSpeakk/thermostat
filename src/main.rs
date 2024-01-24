#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")] // Hide console window on Windows if we're not debugging.
#![allow(non_snake_case)] // The project name is also the name of the process, which should have a capital T.

use slint::{private_unstable_api::re_exports::{EventResult, KeyEvent}, WindowPosition, PhysicalPosition};
use tokio::task::JoinHandle;
use std::{fs, path::Path, io::{BufWriter, Write}, time::{Duration, Instant}};

slint::include_modules!();

const API_URL: &str = "http://192.168.178.48:5567/";
const OPTIONS_PATH: &str = "options.json";

type AnyError = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    // Read options from disk.
    let options = 
        if Path::new(OPTIONS_PATH).exists() { Some(fs::read_to_string(OPTIONS_PATH)?) }
        else { None };
    let options = options.map_or(Options::default(), |s| serde_json::from_str(&s).unwrap());

    // Get initial config and states from API.
    let api_resp = get_api_async(true).await?;

    // Run the UI.
    run_ui(AppWindow::new()?, api_resp, options).await
}

/// Registers event handlers and runs the UI.
async fn run_ui(ui: AppWindow, resp: APIResponse, mut options: Options) -> Result<(), AnyError> {
    let singletons = ui.global::<Singletons>();
    singletons.set_config(resp.config.unwrap().into()); // Set initial config
    singletons.set_state(resp.into()); // Set initial heating state

    // Register event handlers
    register_target_temp_handler(&ui);
    register_window_move_handler(&ui);
    register_quit_handler(&ui);
    register_key_handler(&ui);

    start_ui_updater(&ui);

    // Restore previous window position
    ui.window().set_position(WindowPosition::Physical(options.window_pos));
    ui.run()?;
    
    // Save options upon shutdown.
    options.window_pos = ui.window().position();
    save_options(&options)?;

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
        ui.window().set_position(slint::WindowPosition::Physical(slint::PhysicalPosition { x: pos.x + dx, y: pos.y + dy }));
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
                    cfg.target_temp += 0.5;
                });
                EventResult::Accept
            },
            "\u{f701}" => { // Down arrow
                modify_config(&ui, |cfg: &mut ThermostatConfig| {
                    cfg.target_temp -= 0.5;
                });
                EventResult::Accept
            },
            _ => EventResult::Reject
        }
    });
}

fn start_ui_updater(ui: &AppWindow) {
    // Periodically update the UI with the latest data from the API.
    let ui_handle = ui.as_weak();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            interval.tick().await; // Run every 15 seconds

            match get_api_async(false).await {
                Ok(resp) => {
                    // Ignore result, we don't care if it actually updated.
                    // If it didn't, the UI is probably gone anyway.
                    let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                        ui.global::<Singletons>().set_state(resp.into());
                    });
                },
                Err(err) => eprintln!("Could not get metrics from API: {:?}", err),
            }
        }
    });
}

/// Writes the options to disk in JSON format.
fn save_options(options: &Options) -> Result<(), AnyError> {
    let mut writer = BufWriter::new(fs::File::create(OPTIONS_PATH)?);
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
            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                // Update state
                ui.global::<Singletons>().set_state(resp.into());
            });
        }
    });
}

/// Send a PATCH request to the API.
async fn patch_api_async(client: &reqwest::Client, new_config: ThermostatConfig) -> Result<APIResponse, reqwest::Error> {
    println!("Updating config to {:?}", new_config);

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
    config: Option<ThermostatConfig>,
    temperature: f32,
    co2: i32,
    is_heating: bool,
}

impl From<APIResponse> for State {
    fn from(resp: APIResponse) -> Self {
        Self {
            current_temp: resp.temperature,
            co2: resp.co2,
            is_heating: resp.is_heating,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct Options {
    #[serde(with = "PhysicalPositionRemote")]
    window_pos: PhysicalPosition,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            window_pos: PhysicalPosition { x: 190, y: 190 }
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(remote = "PhysicalPosition")]
struct PhysicalPositionRemote {
    x: i32,
    y: i32,
}

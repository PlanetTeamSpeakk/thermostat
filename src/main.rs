// #![windows_subsystem = "windows"] // Uncomment this line to hide the console window on Windows.

use std::collections::HashMap;
use slint::private_unstable_api::re_exports::{EventResult, KeyEvent};

slint::include_modules!();

const API_URL: &str = "http://192.168.178.66:5566/";
const METRICS_URL: &str = "http://192.168.178.66:8000/";

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;

    let cfg_res = get_config_async().await;
    match cfg_res {
        Ok(cfg) => run_ui(ui, cfg).await,
        Err(e) => Ok(eprintln!("Could not get config from API: {:?}", e)),
    }
}

async fn run_ui(ui: AppWindow, cfg: ThermostatConfig) -> Result<(), slint::PlatformError> {
    ui.global::<Logic>().set_config(cfg.into()); // Set initial config

    // Handle target temp updates.
    let ui_handle = ui.as_weak();
    ui.on_request_config_change(move || {
        let ui = ui_handle.unwrap();
        patch_api(&reqwest::Client::new(), ui.global::<Logic>().get_config().into()); // Send PATCH request to API
    });

    // Window move event handler
    let ui_handle = ui.as_weak();
    ui.on_request_window_move(move |dx: i32, dy: i32| {
        let ui = ui_handle.unwrap();
        let pos = ui.window().position(); // Current position

        // Move the window along with the cursor.
        ui.window().set_position(slint::WindowPosition::Physical(slint::PhysicalPosition { x: pos.x + dx, y: pos.y + dy }));
    });

    // Window close event handler
    let ui_handle = ui.as_weak();
    ui.on_request_quit(move || {
        let ui = ui_handle.unwrap();
        let _ = ui.window().hide(); // We do not care about the result here.
    });

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

    let ui_handle = ui.as_weak();
    ui.on_window_init(move || {
        let ui = ui_handle.unwrap();
        println!("UI initialized!");
        ui.window();
    });

    let ui_handle = ui.as_weak();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            interval.tick().await; // Run every 15 seconds

            match get_metrics_async().await {
                Ok(metrics) => {
                    // Ignore result, we don't care if it actually updated.
                    // If it didn't, the UI is probably gone anyway.
                    let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                        // Update temperature
                        let temp = metrics.get("temperature").unwrap_or(&"0".to_string()).parse::<f32>().unwrap();
                        ui.set_current_temp(temp);
                    });
                },
                Err(err) => eprintln!("Could not get metrics from API: {:?}", err),
            }
        }
    });

    ui.run()
}

/// Modify the thermostat config.
fn modify_config(ui: &AppWindow, f: impl FnOnce(&mut ThermostatConfig)) {
    let logic = ui.global::<Logic>(); // Get the logic module.

    let mut cfg: ThermostatConfig = logic.get_config().into(); // Get config.
    f(&mut cfg); // Modify config.
    logic.set_config(cfg.into()); // Set config.

    patch_api(&reqwest::Client::new(), cfg); // Send PATCH request to API
}

/// Send a PATCH request to the API.
/// Any errors are printed to stderr.
fn patch_api(client: &reqwest::Client, new_config: ThermostatConfig) {
    let c: reqwest::Client = client.clone();
    tokio::spawn(async move {
        match patch_api_async(&c, new_config).await {
            Ok(cfg) => println!("API Patch success: {:?}", cfg),
            Err(err) => eprintln!("API Patch error: {:?}", err),
        }
    });
}

async fn patch_api_async(client: &reqwest::Client, new_config: ThermostatConfig) -> Result<ThermostatConfig, reqwest::Error> {
    println!("Updating config to {:?}", new_config);

    client.patch(API_URL)
        .json(&new_config)
        .send()
        .await?
        .json::<ThermostatConfig>()
        .await
}

/// Get the current thermostat config from the API.
async fn get_config_async() -> Result<ThermostatConfig, reqwest::Error> {
    get_api_async().await
}

async fn get_api_async<T>() -> Result<T, reqwest::Error>
where
    T: serde::de::DeserializeOwned {
    reqwest::get(API_URL)
        .await?
        .json::<T>()
        .await
}

/// Get the current metrics from the API as a HashMap of strings to strings.
async fn get_metrics_async() -> Result<HashMap<String, String>, reqwest::Error> {
    let resp = reqwest::get(METRICS_URL).await?.text().await?;
    Ok(resp.split("\n")
        .filter(|s| !s.starts_with("#")) // Filter out comments
        .map(|s| s.split(" ") // Split keys and values
            .map(|s| s.to_string())
            .collect::<Vec<String>>())
        .filter(|v| v.len() == 2) // Filter out invalid lines
        .map(|v| (v[0].clone(), v[1].clone()))
        .collect::<HashMap<String, String>>())
}

// Thermostat config
#[derive(serde::Deserialize, serde::Serialize)]
#[derive(Debug, Clone, Copy)]
struct ThermostatConfig {
    master_switch: bool,
    force: bool,
    target_temp: f32,
    co2_target: Option<f32>,
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
            co2_target: cfg.co2_target.unwrap_or(500.0),
        }
    }
}

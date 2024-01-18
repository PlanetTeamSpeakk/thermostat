// #![windows_subsystem = "windows"] // Uncomment this line to hide the console window on Windows.

use slint::private_unstable_api::re_exports::{EventResult, KeyEvent};

slint::include_modules!();

const API_URL: &str = "http://192.168.178.66:5566/";

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;

    let api_res = get_api_async(true).await;
    match api_res {
        Ok(resp) => run_ui(ui, resp).await,
        Err(e) => Ok(eprintln!("Could not get config from API: {:?}", e)),
    }
}

async fn run_ui(ui: AppWindow, resp: APIResponse) -> Result<(), slint::PlatformError> {
    let singletons = ui.global::<Singletons>();
    singletons.set_config(resp.config.unwrap().into()); // Set initial config
    singletons.set_state(resp.into()); // Set initial heating state

    // Handle target temp updates.
    let ui_handle = ui.as_weak();
    ui.on_request_config_change(move || {
        let ui = ui_handle.unwrap();
        let cfg = ui.global::<Singletons>().get_config().into();

        update_config(&ui, cfg);
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

    ui.run()
}

/// Modify the thermostat config.
fn modify_config(ui: &AppWindow, f: impl FnOnce(&mut ThermostatConfig)) {
    let singletons = ui.global::<Singletons>(); // Get the Singletons module.

    let mut cfg: ThermostatConfig = singletons.get_config().into(); // Get config.
    f(&mut cfg); // Modify config.
    singletons.set_config(cfg.into()); // Set config.

    update_config(ui, cfg);
}

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

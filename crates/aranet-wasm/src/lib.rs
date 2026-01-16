//! WebAssembly module for Aranet sensors via Web Bluetooth

use wasm_bindgen::prelude::*;

/// Initialize the WASM module (called automatically)
#[wasm_bindgen(start)]
pub fn init() {
    // Set up any global state here
    log("Aranet WASM module initialized");
}

/// Greet function exported to JavaScript
#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    let message = format!("Hello, {}! Welcome to Aranet WASM.", name);
    log(&message);
    message
}

/// Log a message to the browser console
#[wasm_bindgen]
pub fn log(message: &str) {
    web_sys::console::log_1(&message.into());
}

// TODO: Future Web Bluetooth integration
// - Connect to Aranet4 devices via Web Bluetooth API
// - Read sensor data (CO2, temperature, humidity, pressure)
// - Parse and expose data to JavaScript

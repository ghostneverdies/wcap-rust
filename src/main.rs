#![windows_subsystem = "windows"]

mod wcap;
use ::wcap::wcap_encoder;
mod region_input;
mod wcap_engine;
mod wcap_tray;
mod wcap_ui;

fn main() {
    unsafe {
        wcap::run();
    }
}

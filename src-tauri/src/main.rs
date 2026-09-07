// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if omarchyinstall_lib::run_storage_helper_if_requested() {
        return;
    }
    omarchyinstall_lib::run()
}

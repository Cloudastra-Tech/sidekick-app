mod audio;
mod backend;
mod call_detect;
mod call_logs;
mod call_summary;
mod capture;
mod checklist;
mod clients;
mod coach;
mod config;
mod consent;
mod cpu;
mod crm;
mod demo;
mod follow_up;
mod history;
mod journal;
mod llm;
mod overlay;
mod playbook;
mod report;
mod settings;
mod stt;
mod tray;
mod trigger;
mod updates;

use tauri::WindowEvent;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(consent::ConsentState::default())
        .manage(capture::CaptureState::default())
        .manage(coach::SummaryState::default())
        .manage(coach::CoachInbox::default())
        .manage(coach::PendingHistory::default())
        .manage(overlay::OverlayState::default())
        .manage(call_detect::CallDetectState::default())
        .manage(updates::PendingUpdate::default())
        .manage(journal::CurrentJournal::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            if let Ok(dir) = tauri::Manager::path(app).app_data_dir() {
                config::init(dir);
            }
            // Calls cut short by a crash or a killed app become saved calls again.
            if let Ok(dir) = history::calls_dir(app.handle()) {
                for id in journal::recover(&dir) {
                    eprintln!("Recovered the transcript of an unfinished call: {id}");
                }
            }
            if let Ok(dir) = tauri::Manager::path(app).resource_dir() {
                config::init_resources(dir);
            }
            tray::build(app.handle())?;
            overlay::apply_glass(app.handle());
            call_detect::spawn(app.handle().clone());
            backend::spawn_stt_key_refresher();
            // Calls the server never got (closed mid-call, or signed out) go up with their logs,
            // once the app has settled.
            let uploader = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                call_logs::upload_missing(uploader).await;
            });
            // Non-fatal: another app may already own one of the shortcuts.
            match overlay::shortcuts_plugin() {
                Ok(plugin) => {
                    if let Err(e) = app.handle().plugin(plugin) {
                        eprintln!("Overlay shortcuts unavailable: {e}");
                    }
                }
                Err(e) => eprintln!("Overlay shortcuts unavailable: {e}"),
            }
            Ok(())
        })
        // Tray app: closing the window hides it; quitting happens from the tray menu.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            consent::acknowledge_consent,
            consent::consent_status,
            capture::start_capture,
            capture::stop_capture,
            capture::set_call_paused,
            capture::start_demo_call,
            capture::capture_status,
            capture::ask_coach,
            coach::last_summaries,
            coach::rate_hint,
            coach::hint_painted,
            coach::provide_call_history,
            backend::warm_connections,
            history::list_calls,
            history::get_call,
            history::delete_call,
            journal::live_transcript,
            follow_up::draft_follow_up,
            call_detect::detected_call,
            call_detect::dismiss_detected_call,
            call_detect::request_start_call,
            report::test_call_report,
            settings::get_settings,
            settings::save_settings,
            settings::save_playbook,
            backend::backend_status,
            backend::backend_login,
            backend::backend_logout,
            backend::backend_sync_settings,
            backend::push_call_to_backend,
            backend::backend_previous_calls,
            clients::client_documents,
            clients::client_upload,
            clients::client_add_note,
            clients::client_update_note,
            clients::client_remove,
            clients::client_context,
            backend::backend_team_calls,
            backend::backend_call_transcript,
            backend::backend_rep_stats,
            backend::backend_refresh_account,
            backend::backend_refresh_stt_key,
            backend::list_members,
            backend::add_member,
            backend::update_member,
            backend::reset_member_password,
            backend::remove_member,
            crm::crm_status,
            crm::send_call_to_crm,
            crm::crm_search_people,
            crm::crm_people_index,
            crm::crm_person_deals,
            crm::crm_person_context,
            settings::list_mics,
            settings::set_mic,
            overlay::overlay_set_expanded,
            updates::check_for_update,
            updates::install_update,
            updates::app_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod commands;
mod config;
mod files;
mod git;
mod github;
mod pty;
mod workspace;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::add_service,
            commands::update_service,
            commands::remove_service,
            commands::clone_service,
            commands::service_clone_status,
            commands::create_group,
            commands::delete_group,
            commands::set_group_members,
            commands::github_is_authenticated,
            commands::github_list_repos,
            commands::list_workspaces,
            commands::create_workspace,
            commands::remove_workspace,
            commands::workspace_status,
            commands::refresh_repo,
            commands::open_in_editor,
            commands::reveal_workspace_folder,
            commands::open_workspace_in_editor,
            files::list_files,
            files::read_file,
            files::write_file,
            files::list_workspace_files,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

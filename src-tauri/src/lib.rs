mod commands;
mod agent;
mod config;
mod files;
mod git;
mod github;
mod integrations;
mod pty;
mod usage;
mod workspace;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            commands::workspace_ai_usage,
            commands::list_base_branches,
            commands::integration_fetch_card,
            commands::workspace_plan_exists,
            commands::generate_plan,
            commands::cancel_plan,
            commands::grill_prompt,
            commands::grill_step,
            commands::generate_plan_decisions,
            commands::plan_tasks,
            commands::set_plan_task,
            commands::git_changes,
            commands::git_commits,
            commands::git_file_diff,
            commands::git_commit_files,
            commands::git_commit_diff,
            commands::get_ai_settings,
            commands::set_ai_settings,
            commands::test_agent,
            commands::list_models,
            commands::integration_status,
            commands::integration_connect,
            commands::integration_disconnect,
            commands::integration_fetch_cards,
            files::list_files,
            files::read_file,
            files::write_file,
            files::list_workspace_files,
            files::move_file,
            files::rename_node,
            files::delete_node,
            files::create_node,
            files::reveal_node,
            files::node_abs_path,
            files::import_files,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod commands;
mod agent;
mod agent_hooks;
mod build;
mod config;
mod files;
mod git;
mod github;
mod health;
mod integrations;
mod links;
mod lsp;
mod proc;
mod pty;
mod ralph;
mod repo;
mod review;
mod search;
mod usage;
mod workspace;

/// `Orbit --orbit-statusline <file>`: status line helper for the Claude
/// sessions Orbit launches (see agent_hooks).
pub fn statusline_helper(file: Option<String>) {
    agent_hooks::statusline_helper(file)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    proc::refresh_path();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Windows: a window created maximized keeps Tauri's undecorated
            // resize strip over its top edge (the strip is only dropped on a
            // WM_SIZE after it's attached), so the corner pixel resizes instead
            // of hitting the close button. The window starts hidden
            // (tauri.windows.conf.json) and is maximized once it exists.
            #[cfg(windows)]
            if let Some(w) = tauri::Manager::get_webview_window(app, "main") {
                let _ = w.maximize();
                let _ = w.show();
            }
            agent_hooks::start(app.handle().clone());
            Ok(())
        })
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
            commands::pr_list,
            commands::pr_search,
            commands::pr_detail,
            commands::pr_file_diff,
            commands::list_workspaces,
            commands::create_workspace,
            commands::create_workspace_from_branch,
            commands::pr_checkout_plan,
            commands::pr_checkout,
            commands::ws_pull,
            review::pr_review_data,
            review::pr_add_comment,
            review::pr_reply,
            review::pr_edit_comment,
            review::pr_delete_comment,
            review::pr_resolve_thread,
            review::pr_submit_review,
            ralph::ralph_state,
            ralph::ralph_save_prd,
            ralph::ralph_save_prompt,
            ralph::ralph_interview,
            ralph::ralph_generate_prd,
            ralph::ralph_cancel_generate,
            ralph::ralph_start,
            ralph::ralph_stop,
            ralph::ralph_running,
            ralph::ralph_pause,
            ralph::ralph_runs,
            ralph::ralph_run_events,
            commands::build_repo,
            commands::cancel_build,
            commands::health_check,
            commands::update_service_settings,
            commands::get_folders,
            commands::reveal_service,
            commands::set_folders,
            commands::remove_workspace,
            commands::ws_owner_repo,
            commands::ws_address_review,
            commands::race_create_variant,
            commands::race_variant_stats,
            commands::race_adopt,
            commands::race_discard,
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
            commands::ws_commit_message,
            commands::ws_commit,
            commands::ws_push,
            commands::ws_rebase,
            commands::ws_resolve_conflicts,
            commands::ws_rebase_continue,
            commands::ws_rebase_abort,
            commands::ws_create_pr,
            commands::ws_create_prs,
            commands::ws_pr_draft,
            commands::ws_prs,
            commands::ws_prs_flat,
            commands::ws_pr_status,
            commands::ws_pr_merge,
            repo::repo_overview,
            repo::repo_switch,
            repo::repo_create_branch,
            repo::repo_delete_branch,
            repo::repo_pull,
            repo::repo_commit,
            repo::repo_discard,
            repo::repo_stash,
            repo::repo_stash_pop,
            repo::repo_stash_drop,
            repo::repo_undo_commit,
            repo::repo_history,
            repo::repo_branch_prs,
            repo::repo_prune_worktrees,
            repo::repo_briefs,
            repo::repo_compare,
            repo::repo_cherry_pick,
            repo::repo_revert,
            repo::repo_operation,
            commands::ws_checks,
            commands::check_rerun,
            commands::check_logs,
            commands::investigate_check,
            commands::apply_check_fix,
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
            files::log_render_crash,
            files::write_file,
            files::list_workspace_files,
            files::move_file,
            files::rename_node,
            files::delete_node,
            files::create_node,
            files::reveal_node,
            files::node_abs_path,
            files::import_files,
            lsp::lsp_start,
            lsp::lsp_send,
            lsp::lsp_stop,
            lsp::lsp_status,
            lsp::lsp_available,
            lsp::lsp_log,
            lsp::lsp_configure,
            lsp::lsp_read_file,
            lsp::lsp_cpp_setup,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            search::search_workspace,
            pty::pty_scrollback,
            pty::pty_forget,
            pty::claude_session_exists,
            agent_hooks::claude_rate_limits,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_, event| {
            // Language servers are Orbit's children: don't leave them running.
            if let tauri::RunEvent::Exit = event {
                lsp::shutdown_all();
            }
        });
}

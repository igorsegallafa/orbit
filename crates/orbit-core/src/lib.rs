//! Orbit's backend: config, workspaces and worktrees, git and GitHub, language
//! servers, file watching and the rest of the logic both UIs share. Nothing
//! here depends on Tauri; events go through [`events::EventSink`].
pub mod config;
pub mod events;
pub mod git;
pub mod github;
pub mod health;
pub mod integrations;
pub mod links;
pub mod lsp;
pub mod notes;
pub mod proc;
pub mod repo;
pub mod review;
pub mod search;
pub mod usage;
pub mod watch;
pub mod workspace;

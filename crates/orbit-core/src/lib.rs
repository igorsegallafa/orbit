//! Orbit's backend: config, workspaces and worktrees, git and GitHub, language
//! servers, file watching and the rest of the logic under the UI. Nothing
//! here depends on GPUI; events go through [`events::EventSink`].
pub mod agent;
pub mod agent_hooks;
pub mod build;
pub mod cmake_api;
pub mod config;
pub mod crash;
pub mod dap;
pub mod events;
pub mod files;
pub mod git;
pub mod github;
pub mod health;
pub mod integrations;
pub mod links;
pub mod lsp;
pub mod notes;
pub mod proc;
pub mod prs;
pub mod pty;
pub mod ralph;
pub mod repo;
pub mod review;
pub mod run;
pub mod search;
pub mod settings;
pub mod usage;
pub mod updates;
pub mod watch;
pub mod workspace;

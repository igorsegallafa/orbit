// Tauri commands for Ralph (orbit_core::ralph).
use crate::{blocking, events};
use orbit_core::agent::GrillRound;
use orbit_core::ralph::{self, prd::Prd, RalphState, RunConfig};
use serde_json::Value;
use tauri::AppHandle;

#[tauri::command]
pub async fn ralph_state(workspace: String, repo: String) -> Result<RalphState, String> {
    blocking(move || ralph::state(workspace, repo)).await
}

#[tauri::command]
pub async fn ralph_save_prd(workspace: String, repo: String, prd: Prd) -> Result<(), String> {
    blocking(move || ralph::save_prd(workspace, repo, prd)).await
}

/// Saves the workspace's iteration prompt; None/empty restores the default.
#[tauri::command]
pub async fn ralph_save_prompt(workspace: String, prompt: Option<String>) -> Result<(), String> {
    blocking(move || ralph::save_prompt(workspace, prompt)).await
}

/// One round of the PRD clarifying interview (same stepper as Plan's grill).
#[tauri::command]
pub async fn ralph_interview(
    workspace: String,
    repo: String,
    brief: String,
    answers: Vec<(String, String)>,
    rounds_done: usize,
    max_rounds: Option<usize>,
) -> Result<GrillRound, String> {
    blocking(move || ralph::interview(workspace, repo, brief, answers, rounds_done, max_rounds)).await
}

/// Writes the PRD with the agent (streams `ralph-prd-progress`).
#[tauri::command]
pub async fn ralph_generate_prd(
    app: AppHandle,
    workspace: String,
    repo: String,
    brief: String,
    decisions: String,
) -> Result<Prd, String> {
    blocking(move || ralph::generate_prd(events(app), workspace, repo, brief, decisions)).await
}

#[tauri::command]
pub async fn ralph_cancel_generate(workspace: String, repo: String) -> Result<bool, String> {
    ralph::cancel_generate(workspace, repo)
}

#[tauri::command]
pub async fn ralph_start(app: AppHandle, workspace: String, repo: String, config: RunConfig) -> Result<(), String> {
    ralph::start(events(app), workspace, repo, config)
}

/// Ralph over the feature plan: the plan's tasks (as a PRD) and the run.
#[tauri::command]
pub async fn ralph_plan_state(workspace: String) -> Result<RalphState, String> {
    blocking(move || ralph::plan_state(workspace)).await
}

/// Starts Ralph on the plan's open tasks (key `<workspace>/plan`; stop,
/// pause and history take repo "plan").
#[tauri::command]
pub async fn ralph_plan_start(app: AppHandle, workspace: String, config: RunConfig) -> Result<(), String> {
    blocking(move || ralph::plan_start(events(app), workspace, config)).await
}

/// Keys (`workspace/repo`) of the runs in progress, for app-wide indicators.
#[tauri::command]
pub async fn ralph_running() -> Result<Vec<String>, String> {
    ralph::running()
}

/// Stops now: kills the running iteration (its commits stay).
#[tauri::command]
pub async fn ralph_stop(workspace: String, repo: String) -> Result<(), String> {
    ralph::stop(workspace, repo)
}

/// Stops after the current iteration finishes.
#[tauri::command]
pub async fn ralph_pause(workspace: String, repo: String) -> Result<(), String> {
    ralph::pause(workspace, repo)
}

/// Past runs of this workspace (newest first), from their run.json.
#[tauri::command]
pub async fn ralph_runs(workspace: String) -> Result<Vec<Value>, String> {
    blocking(move || ralph::past_runs(workspace)).await
}

/// Events of a past run (events.jsonl), for replaying its feed.
#[tauri::command]
pub async fn ralph_run_events(workspace: String, id: String) -> Result<Vec<Value>, String> {
    blocking(move || ralph::run_events(workspace, id)).await
}

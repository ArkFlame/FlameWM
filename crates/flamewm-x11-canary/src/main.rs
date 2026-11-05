//! Xephyr canary harness: locate Flame surfaces, query geometry/map state,
//! send synthetic button/motion events, sample pixels, emit JSON/text.
//!
//! Read-only probe plus synthetic input; never touches product renderers.
//! Scenarios: SR05 first-present, SR07 popup matrix, SR08 black-pixel,
//! SR15 input matrix, J12 focused assertions (panel pre-input, start
//! stacking, selection mid-drag, desktop right-click, shell context
//! popup, chrome geometry), J13 window-manager contracts (latency,
//! activation, popups, cursors, hints), J06 post-registration probes
//! (calendar-300, start single-surface/app-click, desktop icon wake,
//! filemanager map-latency, title-centered, resize-8, fixed-size,
//! pixmap-ledger).

mod canary;
mod cli;
mod commands;
mod contracts;
mod output;
mod scenarios;

pub(crate) use canary::{Canary, WinInfo};
use cli::usage;
pub(crate) use cli::{arg, has};
use commands::{
    interaction_perf::cmd_interaction_perf,
    j06::cmd_j06,
    j12::cmd_j12,
    j13::cmd_j13,
    probe::{
        cmd_click, cmd_click_at, cmd_geometry, cmd_list, cmd_map_state, cmd_motion,
        cmd_root_geometry, cmd_root_shot, cmd_sample, cmd_sample_root, cmd_stacking,
    },
    profile::{cmd_profile_idle, cmd_profile_pss, cmd_profile_startup, cmd_profile_window},
    sr::{cmd_sr05, cmd_sr07, cmd_sr08, cmd_sr15},
};
use output::fail;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage();
    }
    let cmd = args[1].clone();
    let rest = args[1..].to_vec();
    // wait-mapped is an alias of sr05 with require semantics.
    let canary = match Canary::connect() {
        Ok(c) => c,
        Err(e) => fail(&format!("X connect failed: {e}")),
    };
    match cmd.as_str() {
        "list" => cmd_list(&canary, &rest),
        "geometry" => cmd_geometry(&canary, &rest),
        "map-state" => cmd_map_state(&canary, &rest),
        "click" => cmd_click(&canary, &rest),
        "motion" => cmd_motion(&canary, &rest),
        "sample" => cmd_sample(&canary, &rest),
        "sample-root" => cmd_sample_root(&canary, &rest),
        "stacking" => cmd_stacking(&canary, &rest),
        "root-geometry" => cmd_root_geometry(&canary, &rest),
        "root-shot" => cmd_root_shot(&canary, &rest),
        "click-at" => cmd_click_at(&canary, &rest),
        "wait-mapped" | "sr05" => cmd_sr05(&canary, &rest),
        "profile-startup" => cmd_profile_startup(&canary, &rest),
        "profile-pss" => cmd_profile_pss(&rest),
        "profile-idle" => cmd_profile_idle(&rest),
        "profile-window" => cmd_profile_window(&canary, &rest),
        "sr07" => cmd_sr07(&canary, &rest),
        "sr08" => cmd_sr08(&canary, &rest),
        "sr15" => cmd_sr15(&canary, &rest),
        "j12" => cmd_j12(&canary, &rest),
        "j13" => cmd_j13(&canary, &rest),
        "j06" => cmd_j06(&canary, &rest),
        "interaction-perf" => cmd_interaction_perf(&canary, &rest),
        "j07" => scenarios::cmd_j07(&canary, &rest),
        _ => usage(),
    }
    ExitCode::SUCCESS
}

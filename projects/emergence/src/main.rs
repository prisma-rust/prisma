//! Emergence desktop executable entry point.

#![deny(warnings)]

use emergence_core::log_persist::init_session_file_logger;
use emergence_core::perf_report::{
    cli_usage_text, install_cli_config, parse_cli_args, CliAction,
};

fn main() {
    match parse_cli_args(std::env::args()) {
        Ok(CliAction::PrintHelpAndExit) => {
            println!("{}", cli_usage_text());
        }
        Ok(CliAction::Run(cfg)) => {
            if let Some(log_path) = init_session_file_logger() {
                eprintln!("[emergence][log] writing session log -> {}", log_path.display());
            }
            if cfg.is_enabled() {
                eprintln!(
                    "[emergence][perf] performance telemetry active (warmup={} frames); report will be saved on exit",
                    cfg.warmup_frames
                );
            }
            install_cli_config(cfg);
            emergence_core::build_app().run();
        }
        Err(err) => {
            eprintln!("error: {err}\n\n{}", cli_usage_text());
            std::process::exit(2);
        }
    }
}

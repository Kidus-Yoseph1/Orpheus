use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Orpheus — a beautiful private audiobook reader that lives in the terminal.
#[derive(Debug, Parser)]
#[command(name = "orpheus", version, about)]
pub struct Cli {
    /// Book file (.epub) or directory to open. Omit for library home.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Color theme (midnight, paper, sepia, dark, forest, minimal).
    #[arg(long)]
    pub theme: Option<String>,

    /// Reader style (classic, paper, sepia, midnight, focus, minimal).
    #[arg(long)]
    pub style: Option<String>,

    /// Voice id to select at startup.
    #[arg(long)]
    pub voice: Option<String>,

    /// TTS model id to select at startup.
    #[arg(long)]
    pub model: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Open the voice manager.
    Voices,
    /// Open the model manager (install / select / pull on the fly).
    Models,
    /// Print config path and current settings.
    Config,
}

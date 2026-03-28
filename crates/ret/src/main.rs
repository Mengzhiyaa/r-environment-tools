// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ret::{
    find_and_report_installations_stdio, jsonrpc::start_jsonrpc_server, resolve_report_stdio,
    FindOptions,
};
use ret_core::{output::OutputSchema, r_installation::RInstallationKind};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Finds R installations and reports them to standard output.
    Find {
        /// Files or directories to search.
        #[arg(value_name = "SEARCH PATHS")]
        search_paths: Option<Vec<PathBuf>>,

        /// List the installations found.
        #[arg(short, long)]
        list: bool,

        /// Directory to cache resolved R installation details.
        #[arg(short, long, env = "RET_CACHE_DIRECTORY")]
        cache_directory: Option<PathBuf>,

        /// Display verbose output.
        #[arg(short, long)]
        verbose: bool,

        /// Limit the search to a specific installation kind.
        #[arg(short, long)]
        kind: Option<RInstallationKind>,

        /// Output results as JSON.
        #[arg(short, long)]
        json: bool,

        /// Path to the rig executable.
        #[arg(long, env = "RET_RIG_EXECUTABLE")]
        rig_executable: Option<PathBuf>,

        /// Path to the conda, mamba, or micromamba executable.
        #[arg(long, env = "RET_CONDA_EXECUTABLE")]
        conda_executable: Option<PathBuf>,

        /// JSON output schema.
        #[arg(long, env = "RET_OUTPUT_SCHEMA", default_value = "ret")]
        output_schema: OutputSchema,
    },
    /// Resolves a single R installation from an executable or home directory.
    Resolve {
        /// Fully qualified path to the R executable or installation directory.
        #[arg(value_name = "R EXE")]
        executable: PathBuf,

        /// Directory to cache resolved R installation details.
        #[arg(short, long, env = "RET_CACHE_DIRECTORY")]
        cache_directory: Option<PathBuf>,

        /// Display verbose output.
        #[arg(short, long)]
        verbose: bool,

        /// Output results as JSON.
        #[arg(short, long)]
        json: bool,

        /// JSON output schema.
        #[arg(long, env = "RET_OUTPUT_SCHEMA", default_value = "ret")]
        output_schema: OutputSchema,
    },
    /// Starts the JSON-RPC server.
    Server,
}

fn main() {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Find {
        list: true,
        verbose: false,
        search_paths: None,
        cache_directory: None,
        kind: None,
        json: false,
        rig_executable: None,
        conda_executable: None,
        output_schema: OutputSchema::Ret,
    }) {
        Commands::Find {
            list,
            verbose,
            search_paths,
            cache_directory,
            kind,
            json,
            rig_executable,
            conda_executable,
            output_schema,
        } => {
            find_and_report_installations_stdio(FindOptions {
                print_list: list,
                print_summary: true,
                verbose,
                search_paths,
                cache_directory,
                kind,
                json,
                rig_executable,
                conda_executable,
                output_schema,
            });
        }
        Commands::Resolve {
            executable,
            cache_directory,
            verbose,
            json,
            output_schema,
        } => resolve_report_stdio(executable, verbose, cache_directory, json, output_schema),
        Commands::Server => start_jsonrpc_server(),
    }
}

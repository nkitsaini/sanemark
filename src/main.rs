//! Entry point.
//!
//! With no subcommand the binary serves the Markdown language server over
//! stdio. The `format` and `lint` subcommands run the formatter / broken-link
//! checker from the command line (see [`sanemark::cli`]).

use std::process::ExitCode;

use sanemark::cli;
use sanemark::server::Backend;
use tower_lsp_server::{LspService, Server};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("format") | Some("inline") | Some("lint") | Some("check") | Some("config")
        | Some("readme") => ExitCode::from(cli::run(&args[1..]) as u8),
        Some("-h") | Some("--help") | Some("help") => {
            cli::print_help();
            ExitCode::SUCCESS
        }
        Some("-V") | Some("--version") => {
            println!("sanemark {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Default (and explicit `serve`/`lsp`): speak LSP over stdio.
        Some("serve") | Some("lsp") | None => {
            serve();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("sanemark: unknown command '{other}'\n");
            cli::print_help();
            ExitCode::from(2)
        }
    }
}

/// Run the language server over stdio.
fn serve() {
    let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::new(Backend::new);
        Server::new(stdin, stdout, socket).serve(service).await;
    });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        eprintln!(
            "nades-mcp: local MCP stdio server\n\nOptions:\n  --sandbox-binary PATH  Compiled nades-sandbox (default: sibling executable)\n  --packs-dir PATH       Local map cache (default: ~/.cache/nades/packs)\n  --runs-dir PATH        Local logs (default: ~/.cache/nades/sandbox/mcp)\n\nStart with app_status and maps_list. app_start opens an owned window; view_render makes a headless image.\nDisconnect hands a ready window to the user. app_stop closes it explicitly."
        );
        return Ok(());
    }
    nades_mcp::serve(nades_mcp::Config::from_args(args)?).await
}

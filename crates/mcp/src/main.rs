//! `nikgob-mcp`: the stdio MCP server as a console program, beside the
//! player (the same as `nikgob mcp`).

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).target(env_logger::Target::Stderr).init();
    std::process::exit(nikgob_mcp::stdio::main());
}

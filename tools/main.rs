mod bootstrap;
mod dependency_policy;

type ToolResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> ToolResult {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "bootstrap" => bootstrap::run().await,
        [command] if command == "dependency-policy" => dependency_policy::run(),
        _ => Err("usage: lab-tool <bootstrap|dependency-policy>".into()),
    }
}

mod bootstrap;
mod dependency_policy;
mod delivery;
mod delivery_policy;
#[path = "../src/configuration.rs"]
mod configuration;

type ToolResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> ToolResult {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "bootstrap" => bootstrap::run().await,
        [command] if command == "dependency-policy" => dependency_policy::run(),
        [command] if command == "probe" => delivery::probe().await,
        [command] if command == "acceptance" => delivery::acceptance().await,
        [command, directory, image] if command == "init-env" => delivery::initialize(directory, image),
        [command, rest @ ..] if command == "delivery-policy" => delivery_policy::run(rest),
        _ => Err("usage: lab-tool <bootstrap|dependency-policy|probe|acceptance|init-env DIRECTORY IMAGE-DIGEST|delivery-policy ...>".into()),
    }
}

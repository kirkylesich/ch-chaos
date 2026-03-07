use clap::Parser;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Mode {
    Operator,
    Runner,
}

#[derive(Parser, Debug)]
#[command(name = "chimp-chaos", about = "Kubernetes chaos engineering operator")]
struct Cli {
    #[arg(long)]
    mode: Mode,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    match cli.mode {
        Mode::Operator => {
            tracing::info!("starting chimp-chaos in operator mode");
            let client = kube::Client::try_default().await?;
            let prometheus_url = std::env::var("PROMETHEUS_URL")
                .unwrap_or_else(|_| chimp_chaos::operator::types::DEFAULT_PROMETHEUS_URL.to_string());
            chimp_chaos::operator::controller::run(client, &prometheus_url).await?;
        }
        Mode::Runner => {
            tracing::info!("starting chimp-chaos in runner mode");
            let config = chimp_chaos::runner::entry::RunnerConfig::from_env()?;
            chimp_chaos::runner::entry::run(config).await?;
        }
    }

    Ok(())
}

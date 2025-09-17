use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use hanzo_model_discovery::{HanzoModelDiscovery, ModelDiscovery, ModelFilter, ModelSource, SortBy};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::fs;
use tracing::{info, warn};

#[derive(Parser)]
#[command(name = "hanzoai")]
#[command(author = "Hanzo AI Team")]
#[command(version = "1.1.10")]
#[command(about = "Hanzo AI Engine - High-performance LLM inference and model management", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Verbosity level
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Search for models
    Search {
        /// Search query
        query: Option<String>,

        /// Filter by source (hanzo-lm, hanzo-mlx, hanzo-community, etc.)
        #[arg(short, long)]
        source: Option<Vec<String>>,

        /// Only show trusted models
        #[arg(short, long)]
        trusted: bool,

        /// Maximum model size in GB
        #[arg(long)]
        max_size: Option<f32>,

        /// Minimum context length
        #[arg(long)]
        min_context: Option<u32>,

        /// Number of results to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Pull a model from HuggingFace
    Pull {
        /// Model ID (e.g., hanzo-lm/Llama-3.3-70B-Instruct)
        model: String,

        /// Local path to save the model
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Force re-download even if exists
        #[arg(short, long)]
        force: bool,
    },

    /// List locally available models
    List {
        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,
    },

    /// Run inference server
    Serve {
        /// Model to serve
        #[arg(short, long)]
        model: Option<String>,

        /// Port to listen on
        #[arg(short, long, default_value = "36900")]
        port: u16,

        /// Host to bind to
        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// Number of GPU layers to offload
        #[arg(long)]
        gpu_layers: Option<u32>,

        /// Context size
        #[arg(long, default_value = "8192")]
        context: u32,
    },

    /// Get recommended models for a use case
    Recommend {
        /// Use case: chat, code, embedding, vision, mlx
        use_case: String,
    },

    /// Manage model repositories
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },

    /// Show model information
    Info {
        /// Model ID
        model: String,
    },
}

#[derive(Subcommand)]
enum RepoCommands {
    /// Mirror models from a source to Hanzo repos
    Mirror {
        /// Source organization (e.g., lmstudio-community)
        source: String,

        /// Target organization (e.g., hanzo-community)
        target: String,

        /// Dry run - don't actually fork
        #[arg(long)]
        dry_run: bool,
    },

    /// Sync all Hanzo repositories
    Sync {
        /// Only sync specific organizations
        #[arg(short, long)]
        org: Option<Vec<String>>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let log_level = match cli.verbose {
        0 => "error",
        1 => "warn",
        2 => "info",
        3 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .init();

    match cli.command {
        Commands::Search {
            query,
            source,
            trusted,
            max_size,
            min_context,
            limit,
        } => {
            handle_search(query, source, trusted, max_size, min_context, limit).await?
        }
        Commands::Pull { model, output, force } => {
            handle_pull(&model, output, force).await?
        }
        Commands::List { detailed } => handle_list(detailed).await?,
        Commands::Serve { model, port, host, gpu_layers, context } => {
            handle_serve(model, port, &host, gpu_layers, context).await?
        }
        Commands::Recommend { use_case } => handle_recommend(&use_case).await?,
        Commands::Repo { command } => handle_repo(command).await?,
        Commands::Info { model } => handle_info(&model).await?,
    }

    Ok(())
}

async fn handle_search(
    query: Option<String>,
    source: Option<Vec<String>>,
    trusted: bool,
    max_size: Option<f32>,
    min_context: Option<u32>,
    limit: usize,
) -> Result<()> {
    println!("{}", "🔍 Searching models...".bright_blue().bold());

    let discovery = HanzoModelDiscovery::new();

    let sources = source.map(|srcs| {
        srcs.into_iter()
            .map(|s| match s.as_str() {
                "hanzo-lm" => ModelSource::HanzoLM,
                "hanzo-mlx" => ModelSource::HanzoMLX,
                "hanzo-community" => ModelSource::HanzoCommunity,
                "hanzo-embeddings" => ModelSource::HanzoEmbeddings,
                "hanzo-tools" => ModelSource::HanzoTools,
                "lmstudio" => ModelSource::LMStudio,
                "mlx-community" => ModelSource::MLXCommunity,
                other => ModelSource::HuggingFace(other.to_string()),
            })
            .collect()
    });

    let filter = ModelFilter {
        source: sources,
        search_query: query,
        trusted_only: trusted,
        max_size_gb: max_size,
        min_context,
        ..Default::default()
    };

    let models = discovery.search_models(&filter, SortBy::Downloads, limit).await?;

    if models.is_empty() {
        println!("{}", "No models found matching your criteria.".yellow());
        return Ok(());
    }

    println!("\n{}", format!("Found {} models:", models.len()).green().bold());
    println!("{}", "─".repeat(80).bright_black());

    for (i, model) in models.iter().enumerate() {
        let trust_badge = if model.trusted_source {
            "✓".green()
        } else {
            "?".yellow()
        };

        let size_str = model
            .model_size_gb
            .map(|s| format!("{:.1}GB", s))
            .unwrap_or_else(|| "?".to_string());

        let quant_str = model
            .quantization
            .as_ref()
            .map(|q| format!("[{}]", q))
            .unwrap_or_else(|| String::new());

        println!(
            "{}. {} {} {} {}",
            (i + 1).to_string().bright_black(),
            trust_badge,
            model.id.bright_white().bold(),
            quant_str.bright_cyan(),
            format!("({})", size_str).bright_black()
        );

        if let Some(params) = &model.parameters {
            print!("   {} ", params.yellow());
        }
        if let Some(ctx) = model.context_length {
            print!("{}k context ", (ctx / 1024).to_string().green());
        }
        println!(
            "⬇ {} ❤ {}",
            humansize::format_size(model.downloads, humansize::BINARY),
            model.likes
        );
    }

    Ok(())
}

async fn handle_pull(model: &str, output: Option<PathBuf>, force: bool) -> Result<()> {
    println!("{}", format!("📥 Pulling model: {}", model).bright_blue().bold());

    // Determine output path
    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;

    let models_dir = output.unwrap_or_else(|| {
        home.home_dir().join(".hanzo").join("models")
    });

    fs::create_dir_all(&models_dir).await?;

    let model_path = models_dir.join(model.replace('/', "--"));

    if model_path.exists() && !force {
        println!("{}", "✓ Model already exists. Use --force to re-download.".yellow());
        return Ok(());
    }

    // Create progress bar
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Downloading model files...");

    // Download model using HuggingFace CLI (for now)
    let output = std::process::Command::new("huggingface-cli")
        .arg("download")
        .arg(model)
        .arg("--local-dir")
        .arg(&model_path)
        .arg("--local-dir-use-symlinks")
        .arg("False")
        .output()?;

    pb.finish_with_message("✓ Download complete!");

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Failed to download model: {}", error));
    }

    println!(
        "{}",
        format!("✅ Model saved to: {}", model_path.display()).green().bold()
    );

    Ok(())
}

async fn handle_list(detailed: bool) -> Result<()> {
    println!("{}", "📦 Local models:".bright_blue().bold());

    let home = directories::UserDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;

    let models_dir = home.home_dir().join(".hanzo").join("models");

    if !models_dir.exists() {
        println!("{}", "No local models found.".yellow());
        return Ok(());
    }

    let mut entries = fs::read_dir(&models_dir).await?;
    let mut models = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            let name = entry.file_name().to_string_lossy().replace("--", "/");
            models.push(name);
        }
    }

    if models.is_empty() {
        println!("{}", "No local models found.".yellow());
        return Ok(());
    }

    println!("{}", "─".repeat(80).bright_black());
    for model in models {
        println!("  • {}", model.bright_white());
    }

    Ok(())
}

async fn handle_serve(
    model: Option<String>,
    port: u16,
    host: &str,
    gpu_layers: Option<u32>,
    context: u32,
) -> Result<()> {
    let model_name = model.unwrap_or_else(|| "hanzo-lm/Llama-3.3-70B-Instruct".to_string());

    println!(
        "{}",
        format!("🚀 Starting Hanzo AI Engine").bright_blue().bold()
    );
    println!("  Model: {}", model_name.yellow());
    println!("  Address: {}:{}", host.green(), port.to_string().green());
    println!("  Context: {}", context.to_string().cyan());
    if let Some(gpu) = gpu_layers {
        println!("  GPU Layers: {}", gpu.to_string().cyan());
    }

    // TODO: Implement actual inference server
    println!("\n{}", "⚠️  Inference server not yet implemented".yellow());
    println!("For now, use: hanzo-node with LLM_PROVIDER configuration");

    Ok(())
}

async fn handle_recommend(use_case: &str) -> Result<()> {
    println!(
        "{}",
        format!("🎯 Recommended models for: {}", use_case).bright_blue().bold()
    );

    let discovery = HanzoModelDiscovery::new();
    let models = discovery.get_recommended(use_case).await?;

    if models.is_empty() {
        println!("{}", "No recommendations found.".yellow());
        return Ok(());
    }

    println!("{}", "─".repeat(80).bright_black());
    for (i, model) in models.iter().enumerate() {
        let size_str = model
            .model_size_gb
            .map(|s| format!("{:.1}GB", s))
            .unwrap_or_else(|| "?".to_string());

        println!(
            "{}. {} {} ({})",
            (i + 1).to_string().bright_black(),
            "★".yellow(),
            model.id.bright_white().bold(),
            size_str.bright_black()
        );

        if let Some(desc) = &model.pipeline_tag {
            println!("   {}", desc.bright_black());
        }
    }

    Ok(())
}

async fn handle_repo(command: RepoCommands) -> Result<()> {
    match command {
        RepoCommands::Mirror { source, target, dry_run } => {
            println!(
                "{}",
                format!("🔄 Mirroring {} → {}", source, target).bright_blue().bold()
            );

            if dry_run {
                println!("{}", "DRY RUN - No changes will be made".yellow());
            }

            // TODO: Implement actual mirroring
            println!("{}", "⚠️  Repository mirroring not yet implemented".yellow());
            println!("Use: ./scripts/fork_community_models.sh");
        }
        RepoCommands::Sync { org } => {
            let orgs = org.unwrap_or_else(|| {
                vec![
                    "hanzo-lm".to_string(),
                    "hanzo-mlx".to_string(),
                    "hanzo-community".to_string(),
                    "hanzo-embeddings".to_string(),
                ]
            });

            println!("{}", "🔄 Syncing repositories:".bright_blue().bold());
            for org in orgs {
                println!("  • {}", org.yellow());
            }

            // TODO: Implement syncing
            println!("{}", "⚠️  Repository sync not yet implemented".yellow());
        }
    }

    Ok(())
}

async fn handle_info(model: &str) -> Result<()> {
    println!("{}", format!("ℹ️  Model info: {}", model).bright_blue().bold());

    let discovery = HanzoModelDiscovery::new();
    let info = discovery.get_model_info(model).await?;

    println!("{}", "─".repeat(80).bright_black());
    println!("ID: {}", info.id.bright_white());
    println!("Author: {}", info.author.yellow());

    if let Some(size) = info.model_size_gb {
        println!("Size: {:.2} GB", size);
    }

    if let Some(quant) = &info.quantization {
        println!("Quantization: {}", quant.cyan());
    }

    if let Some(params) = &info.parameters {
        println!("Parameters: {}", params.green());
    }

    if let Some(ctx) = info.context_length {
        println!("Context: {} tokens", ctx);
    }

    println!("Downloads: {}", humansize::format_size(info.downloads, humansize::BINARY));
    println!("Likes: {}", info.likes);
    println!("URL: {}", info.download_url.bright_blue());

    if info.trusted_source {
        println!("Status: {} Trusted source", "✓".green());
    } else {
        println!("Status: {} Unverified", "⚠".yellow());
    }

    Ok(())
}
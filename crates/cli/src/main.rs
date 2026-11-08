//! `rusticdoc`: the command line, and the service when run with `serve`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand};

#[cfg(target_os = "linux")]
mod init;
mod pdf;
use rusticdoc_core::config::RenderMode;
use rusticdoc_core::job::parse_steps;
use rusticdoc_core::{Config, Document, Engines, Format, Pipeline, RenderRequest, Step};
use rusticdoc_server::openapi::ApiDoc;
use utoipa::OpenApi;

#[derive(Parser)]
#[command(
    name = "rusticdoc",
    version,
    about = "Templates in, documents out. Run `serve` for the HTTP API."
)]
struct Cli {
    /// Log verbosity: -v for debug, -vv for trace.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fill a template and run the steps, writing one file.
    Render(RenderArgs),
    /// Convert one or more documents.
    Convert(ConvertArgs),
    /// Run the HTTP service. Settings come from RUSTICDOC_* variables.
    Serve {
        /// Address to listen on.
        #[arg(long, env = "RUSTICDOC_BIND")]
        bind: Option<String>,
    },
    /// Print the OpenAPI description as JSON.
    Openapi,
    /// Check a running service, for container health checks. Exits 0 when up.
    Health {
        /// The service's address. By default, where this configuration serves.
        #[arg(long)]
        url: Option<String>,
    },
    /// Show the engines this machine has.
    Engines,
    /// PDF operations: merge, split, rotate, flatten, optimize, encrypt,
    /// metadata, bookmarks, embed, watermark, stamp, sign, factur-x, forms.
    Pdf(pdf::PdfArgs),
    /// Check a template against sample data and list every problem.
    Validate {
        template: PathBuf,
        #[arg(short, long)]
        data: Option<PathBuf>,
    },
    /// Print the key a caller's webhook deliveries are signed with, in hex, from
    /// RUSTICDOC_WEBHOOK_SECRET. A receiver decodes it and verifies with it.
    WebhookSecret {
        /// The caller's API key.
        #[arg(long, group = "caller")]
        api_key: Option<String>,
        /// The caller's basic-auth user.
        #[arg(long, group = "caller")]
        basic_user: Option<String>,
        /// The subject of the caller's OpenID Connect tokens.
        #[arg(long, group = "caller")]
        oidc_subject: Option<String>,
    },
    /// Evaluates templates for `serve`, in a process of their own.
    #[command(name = "__render-worker", hide = true)]
    RenderWorker,
}

#[derive(Args)]
struct RenderArgs {
    /// The template file.
    template: PathBuf,
    /// JSON file with the data the template sees.
    #[arg(short, long)]
    data: Option<PathBuf>,
    /// JSON file visible as `ctx`.
    #[arg(long)]
    ctx: Option<PathBuf>,
    /// Steps in order: convert, pdfa, metadata, encrypt, merge, sign. `sign`
    /// comes last and uses RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT.
    #[arg(short, long, default_value = "")]
    steps: String,
    /// Output format for `convert`.
    #[arg(long, default_value = "pdf")]
    to: String,
    /// PDF/A level for `pdfa`.
    #[arg(long, default_value = "PDF/A-2b")]
    pdfa: String,
    /// JSON object for `metadata`.
    #[arg(long)]
    metadata: Option<String>,
    #[arg(long)]
    user_password: Option<String>,
    #[arg(long)]
    owner_password: Option<String>,
    /// Extra PDFs for `merge`.
    #[arg(long = "merge-with")]
    merge_with: Vec<PathBuf>,
    /// An image, stylesheet or font an HTML page names by file name.
    /// Repeat it for each.
    #[arg(long = "asset")]
    assets: Vec<PathBuf>,
    /// What turns HTML into PDF: fulgur or libreoffice.
    #[arg(long, default_value = "fulgur")]
    html_engine: String,
    /// Where to write the result. Defaults to the template's name with the
    /// output extension.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct ConvertArgs {
    /// Documents to convert.
    files: Vec<PathBuf>,
    #[arg(long, default_value = "pdf")]
    to: String,
    /// Export PDF/A at this level instead of plain PDF.
    #[arg(long)]
    pdfa: Option<String>,
    /// Merge all inputs into one PDF.
    #[arg(long)]
    merge: bool,
    /// An image, stylesheet or font an HTML page names by file name.
    /// Repeat it for each.
    #[arg(long = "asset")]
    assets: Vec<PathBuf>,
    /// What turns HTML into PDF: fulgur or libreoffice.
    #[arg(long, default_value = "fulgur")]
    html_engine: String,
    /// Output file (one input or --merge), or directory (several inputs).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Conversion options carrying the HTML engine and the assets read from disk.
fn html_options(
    assets: &[PathBuf],
    engine: &str,
) -> anyhow::Result<rusticdoc_core::convert::ConvertOptions> {
    let mut options = rusticdoc_core::convert::ConvertOptions {
        html_engine: rusticdoc_core::html::HtmlEngine::parse(engine)?,
        ..Default::default()
    };
    let docs = assets
        .iter()
        .map(|p| read_doc(p))
        .collect::<anyhow::Result<Vec<_>>>()?;
    rusticdoc_core::html::check_assets(&docs)?;
    options.assets = docs.into_iter().map(|d| (d.name, d.bytes)).collect();
    Ok(options)
}

fn main() -> anyhow::Result<std::process::ExitCode> {
    let cli = Cli::parse();
    // The worker reads no settings and starts no threads.
    if matches!(cli.command, Command::RenderWorker) {
        std::process::exit(rusticdoc_core::render::worker_main());
    }
    #[cfg(target_os = "linux")]
    if matches!(cli.command, Command::Serve { .. }) && std::process::id() == 1 {
        return init::run();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the runtime")?
        .block_on(run(cli))?;
    Ok(std::process::ExitCode::SUCCESS)
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let mut config = Config::from_env();
    if cli.verbose > 0 {
        config.log_level = if cli.verbose == 1 {
            "debug".into()
        } else {
            "trace".into()
        };
    }

    match cli.command {
        Command::Serve { bind } => {
            if let Err(problem) = config.check() {
                bail!("cannot start: {problem}");
            }
            if let Some(b) = bind {
                config.bind = b;
            }
            // Clients' templates run in workers unless the operator says
            // otherwise, and never fall back on their own.
            if !config.render_mode_set {
                config.render_mode = RenderMode::Isolated;
            }
            if config.render_mode == RenderMode::Isolated && config.render_worker_bin.is_none() {
                config.render_worker_bin =
                    Some(std::env::current_exe().context("find the render worker")?);
            }
            rusticdoc_server::telemetry::init(&config.log_format, &config.log_level);
            let state = rusticdoc_server::AppState::new(config);
            rusticdoc_server::serve(state).await.context("serve")?;
        }
        Command::Openapi => {
            println!("{}", serde_json::to_string_pretty(&ApiDoc::openapi())?);
        }
        Command::Health { url } => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()?;
            // Where this same configuration would serve, so a container told to use
            // another port or root path still passes its own health check.
            let url = url.unwrap_or_else(|| {
                let port = config.bind.rsplit_once(':').map_or(
                    rusticdoc_core::config::DEFAULT_PORT.to_string(),
                    |(_, p)| p.to_string(),
                );
                format!(
                    "http://127.0.0.1:{port}{}",
                    config.root_path.trim_end_matches('/')
                )
            });
            let res = client
                .get(format!("{}/health", url.trim_end_matches('/')))
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    println!("up");
                }
                Ok(r) => bail!("health returned {}", r.status()),
                Err(e) => bail!("health request failed: {e}"),
            }
        }
        Command::Pdf(args) => {
            init_cli_logs(&config);
            pdf::run(&config, args).await?;
        }
        Command::Validate { template, data } => {
            let doc = read_doc(&template)?;
            let data = read_json(data.as_deref())?;
            let problems = rusticdoc_core::template::validate(
                &doc,
                &data,
                &serde_json::Value::Object(Default::default()),
                config.max_render_bytes,
            );
            println!("{}", serde_json::to_string_pretty(&problems)?);
            if problems
                .iter()
                .any(|d| matches!(d.severity, rusticdoc_template::Severity::Error))
            {
                std::process::exit(1);
            }
        }
        Command::RenderWorker => unreachable!("handled in main"),
        Command::WebhookSecret {
            api_key,
            basic_user,
            oidc_subject,
        } => {
            use rusticdoc_server::auth;
            let Some(secret) = &config.webhook_secret else {
                bail!("set RUSTICDOC_WEBHOOK_SECRET to the service's secret");
            };
            let principal = if let Some(key) = api_key {
                auth::key_principal(&key)
            } else if let Some(user) = basic_user {
                auth::basic_principal(&user)
            } else if let Some(subject) = oidc_subject {
                auth::oidc_principal(&subject)
            } else {
                auth::Principal("public".into())
            };
            let key = rusticdoc_server::webhook::signing_key(secret, &principal.0);
            let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
            println!("{hex}");
        }
        Command::Engines => {
            let engines = Engines::from_config(&config);
            println!("{}", serde_json::to_string_pretty(&engines.describe())?);
        }
        Command::Render(args) => {
            init_cli_logs(&config);
            let pipeline = pipeline(&config);
            let template = read_doc(&args.template)?;
            let mut req = RenderRequest::new(template);
            req.data = read_json(args.data.as_deref())?;
            req.ctx = read_json(args.ctx.as_deref())?;
            let mut fields = BTreeMap::new();
            fields.insert("to".into(), args.to.clone());
            fields.insert("pdfa".into(), args.pdfa.clone());
            if let Some(m) = &args.metadata {
                fields.insert("metadata".into(), m.clone());
            }
            if let Some(p) = &args.user_password {
                fields.insert("userPassword".into(), p.clone());
            }
            if let Some(p) = &args.owner_password {
                fields.insert("ownerPassword".into(), p.clone());
            }
            req.steps = parse_steps(&args.steps, &fields)?;
            req.convert_options = html_options(&args.assets, &args.html_engine)?;
            req.extra = args
                .merge_with
                .iter()
                .map(|p| read_doc(p))
                .collect::<anyhow::Result<_>>()?;
            let out = pipeline.run(req).await?;
            let path = args.output.unwrap_or_else(|| PathBuf::from(&out.name));
            std::fs::write(&path, &out.bytes)
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!(
                "wrote {} ({} bytes){}",
                path.display(),
                out.bytes.len(),
                timings(&out.timings)
            );
        }
        Command::Convert(args) => {
            init_cli_logs(&config);
            if args.files.is_empty() {
                bail!("give at least one file to convert");
            }
            let pipeline = pipeline(&config);
            let to = Format::parse(&args.to)?;
            let options = html_options(&args.assets, &args.html_engine)?;
            let step = match &args.pdfa {
                Some(level) => Step::Pdfa {
                    level: rusticdoc_core::job::PdfaLevel::parse(level)?,
                },
                None => Step::Convert { to },
            };
            if args.merge || args.files.len() == 1 {
                let mut req = RenderRequest::new(read_doc(&args.files[0])?);
                req.templating = false;
                req.convert_options = options.clone();
                req.steps = vec![step.clone()];
                if args.merge && args.files.len() > 1 {
                    for f in &args.files[1..] {
                        let mut r = RenderRequest::new(read_doc(f)?);
                        r.templating = false;
                        r.convert_options = options.clone();
                        r.steps = vec![Step::Convert { to: Format::Pdf }];
                        let o = pipeline.run(r).await?;
                        req.extra.push(Document::new(o.name, o.bytes));
                    }
                    req.steps.push(Step::Merge);
                }
                let out = pipeline.run(req).await?;
                let path = args.output.unwrap_or_else(|| PathBuf::from(&out.name));
                std::fs::write(&path, &out.bytes)
                    .with_context(|| format!("write {}", path.display()))?;
                eprintln!(
                    "wrote {} ({} bytes){}",
                    path.display(),
                    out.bytes.len(),
                    timings(&out.timings)
                );
            } else {
                let dir = args.output.unwrap_or_else(|| PathBuf::from("."));
                std::fs::create_dir_all(&dir)?;
                for f in &args.files {
                    let mut req = RenderRequest::new(read_doc(f)?);
                    req.templating = false;
                    req.convert_options = options.clone();
                    req.steps = vec![step.clone()];
                    let out = pipeline.run(req).await?;
                    let path = dir.join(&out.name);
                    std::fs::write(&path, &out.bytes)
                        .with_context(|| format!("write {}", path.display()))?;
                    eprintln!(
                        "wrote {} ({} bytes){}",
                        path.display(),
                        out.bytes.len(),
                        timings(&out.timings)
                    );
                }
            }
        }
    }
    Ok(())
}

fn init_cli_logs(config: &Config) {
    let level = if config.log_level == "info" {
        "warn".to_string()
    } else {
        config.log_level.clone()
    };
    rusticdoc_server::telemetry::init("pretty", &level);
}

fn pipeline(config: &Config) -> Pipeline {
    Pipeline::new(
        std::sync::Arc::new(Engines::from_config(config)),
        config.api_timeout,
    )
    .with_max_output(config.max_render_bytes)
}

fn read_doc(path: &std::path::Path) -> anyhow::Result<Document> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document")
        .to_string();
    Ok(Document::new(name, bytes))
}

fn read_json(path: Option<&std::path::Path>) -> anyhow::Result<serde_json::Value> {
    match path {
        None => Ok(serde_json::Value::Object(Default::default())),
        Some(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
            serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", p.display()))
        }
    }
}

fn timings(t: &[(String, u128)]) -> String {
    if t.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = t.iter().map(|(s, ms)| format!("{s} {ms}ms")).collect();
    format!(" [{}]", parts.join(", "))
}

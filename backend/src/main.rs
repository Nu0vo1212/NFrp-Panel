//! FrpPanel 后端入口：装配依赖 → 初始化数据库 → 启动 HTTP 服务 → 启动采样任务。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use frp_panel::auth::AuthService;
use frp_panel::config::RuntimeConfig;
use frp_panel::crypto::KeyRing;
use frp_panel::deploy::DeployHub;
use frp_panel::state::{AppState, DriverRegistry};

#[derive(Parser, Debug)]
#[command(name = "frp-panel", version, about = "FrpPanel：NFrp / 官方 frp 的统一 Web 管理面板")]
struct Cli {
    /// 配置文件路径（默认 ./panel.toml；也可用环境变量 FRP_PANEL_CONFIG）
    #[arg(short, long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,

    /// 覆盖监听地址（如 0.0.0.0:8787）
    #[arg(long, value_name = "ADDR")]
    bind: Option<String>,

    /// 只打印解析后的配置并退出（排查用）
    #[arg(long)]
    show_config: bool,

    /// 直接重置本地管理员密码：从 stdin 读取新密码，写入本地数据库后退出。
    /// 不需要旧密码，仅用于本机忘记密码时恢复；不会启动 HTTP 服务。
    #[arg(long)]
    reset_admin_password: bool,

    /// 配合 --reset-admin-password 指定用户名；默认取配置中的管理员用户名
    #[arg(long, value_name = "USERNAME")]
    admin_username: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,frp_panel=debug,tower_http=warn,russh=warn")),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    let mut cfg = RuntimeConfig::load(cli.config.clone())?;
    if let Some(bind) = cli.bind {
        cfg.raw.server.bind = bind;
    }

    if cli.show_config {
        println!("{}", toml::to_string_pretty(&cfg.raw)?);
        println!("\n数据目录: {}", cfg.data_dir.display());
        return Ok(());
    }

    // ---- 密钥 ----
    let keys = Arc::new(KeyRing::load_or_create(
        &cfg.jwt_secret_file(),
        &cfg.master_key_file(),
    )?);

    // ---- 数据库 ----
    let db = frp_panel::db::connect(&cfg.database_url()).await?;
    frp_panel::ssh::hostkey::init(db.clone());

    // ---- 本地重置管理员密码（忘记密码时使用） ----
    // 新密码从 stdin 读取，避免出现在命令行 / 进程列表里。
    if cli.reset_admin_password {
        let username = cli
            .admin_username
            .clone()
            .unwrap_or_else(|| cfg.raw.security.initial_admin_username.clone());
        let mut password = String::new();
        std::io::stdin().read_line(&mut password)?;
        let password = password.trim_end_matches(&['\r', '\n'][..]);
        if password.len() < 8 {
            anyhow::bail!("new password must be at least 8 characters");
        }

        let hash = frp_panel::crypto::hash_password(password)?;
        let existing = frp_panel::db::repo::find_user(&db, &username).await?;
        if let Some(user) = existing {
            frp_panel::db::repo::set_password(&db, user.id, &hash).await?;
        } else if frp_panel::db::repo::count_users(&db).await? == 0 {
            // 全新的数据目录：直接以本次输入的口令创建管理员。
            frp_panel::db::repo::insert_user(
                &db,
                &username,
                &hash,
                frp_panel::db::models::Role::Admin,
            )
            .await?;
        } else {
            anyhow::bail!("user not found: {username}");
        }

        // 初始化时生成的随机口令文件已经失效，删除以免误导。
        let initial_file = cfg.initial_admin_password_file();
        if initial_file.exists() {
            let _ = std::fs::remove_file(&initial_file);
        }
        println!("OK");
        return Ok(());
    }

    // ---- 初始管理员 ----
    let initial = frp_panel::auth::ensure_initial_admin(
        &db,
        &cfg.raw.security.initial_admin_username,
        &cfg.raw.security.initial_admin_password,
        &cfg.initial_admin_password_file(),
    )
    .await?;
    if let Some(pw) = initial {
        // 只在控制台提示一次，不写日志文件
        println!("\n================ FrpPanel 初始管理员 ================");
        println!("用户名: {}", cfg.raw.security.initial_admin_username);
        println!("密码:   {pw}");
        println!("已写入 {}", cfg.initial_admin_password_file().display());
        println!("首次登录后请在「设置」中修改密码，并删除该文件。");
        println!("=====================================================\n");
    }

    // ---- 认证与审计 ----
    let auth = Arc::new(AuthService::new(
        keys.jwt_secret().to_vec(),
        cfg.raw.security.access_token_ttl_minutes,
        cfg.raw.security.refresh_token_ttl_days,
        db.clone(),
    ));
    let audit = frp_panel::audit::AuditWriter::new(db.clone(), cfg.raw.audit.enabled);

    let state = Arc::new(AppState {
        cfg: Arc::new(cfg.clone()),
        db: db.clone(),
        keys: keys.clone(),
        auth,
        audit,
        drivers: Arc::new(DriverRegistry::new()),
        deploy: Arc::new(DeployHub::new()),
        poller: Arc::new(tokio::sync::Mutex::new(None)),
    });

    // ---- 后台采样 ----
    let interval = Duration::from_millis(cfg.raw.frp.status_poll_ms.max(2000));
    let handle = frp_panel::metrics::spawn_poller(
        db.clone(),
        state.drivers.clone(),
        state.clone(),
        interval,
        cfg.raw.frp.metrics_history_days,
    );
    *state.poller.lock().await = Some(handle);

    // ---- 路由 ----
    let app = frp_panel::api::routes::build(state.clone())
        .layer(DefaultBodyLimit::max(140 * 1024 * 1024));

    let bind = cfg.raw.server.bind.clone();
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|e| anyhow::anyhow!("无法监听 {bind}：{e}"))?;

    let addr = listener.local_addr()?;
    tracing::info!("FrpPanel {} 已启动：http://{addr}", frp_panel::VERSION);
    tracing::info!(
        "数据目录 {} · 数据库 {}",
        cfg.data_dir.display(),
        cfg.database_url()
    );
    if addr.ip().is_loopback() {
        tracing::info!("面板仅监听回环地址（推荐）；如需外部访问请改用反向代理 + HTTPS");
    } else {
        tracing::warn!("面板监听在非回环地址 {addr}：请务必配置 HTTPS 反向代理与强口令");
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("收到退出信号，正在关闭…");
}

//! sdrd — web-controlled RTL-SDR monitoring daemon.
//!
//! Usage:
//!   sdrd                                  run the server (configured by SDRD_* env vars)
//!   sdrd user add <name> [admin|viewer]   create a user (password read from stdin)
//!   sdrd user passwd <name>               reset a password (read from stdin)
//!   sdrd healthcheck                      exit 0 if the local server answers /api/health

use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

mod adsb;
mod aprs;
mod audio;
mod sstv;
mod auth;
mod bookmarks;
mod config;
mod db;
mod decoders;
mod dsp;
mod events;
mod fkeys;
mod mqtt;
mod orbit;
mod recorder;
mod scheduler;
mod radio;
mod web;

use auth::Role;
use config::Config;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("sdrd=info,tower_http=warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let rt = tokio::runtime::Runtime::new()?;
    match args.as_slice() {
        [] | ["serve"] => rt.block_on(serve()),
        ["user", "add", name] => rt.block_on(user_add(name, Role::Viewer)),
        ["user", "add", name, role] => {
            let role = Role::parse(role).context("role must be admin or viewer")?;
            rt.block_on(user_add(name, role))
        }
        ["user", "passwd", name] => rt.block_on(user_passwd(name)),
        ["healthcheck"] => healthcheck(),
        _ => bail!(
            "usage: sdrd [serve] | sdrd user add <name> [admin|viewer] | \
             sdrd user passwd <name> | sdrd healthcheck"
        ),
    }
}

async fn serve() -> Result<()> {
    let cfg = Config::from_env()?;
    std::fs::create_dir_all(&cfg.data_dir)
        .with_context(|| format!("creating data dir {}", cfg.data_dir.display()))?;
    let db = db::open(&cfg.data_dir.join("sdrd.db")).await?;
    auth::bootstrap_admin(&db, &cfg.admin_user, cfg.admin_password.as_deref()).await?;

    let (iq_tx, _) = broadcast::channel(16);
    let saved_device = db::get_setting::<String>(&db, web::DEVICE_KEY).await.ok().flatten();
    let saved_tcp = db::get_setting::<Vec<String>>(&db, web::TCP_SERVERS_KEY).await.ok().flatten();
    let registry = radio::Registry::new(&cfg, saved_tcp.unwrap_or_default());
    let radio = radio::spawn(
        &cfg,
        radio::Role::Main,
        registry.clone(),
        iq_tx.clone(),
        saved_device,
        radio::main_settings(&cfg),
    )?;
    // The second receiver, for ADS-B ("off" unless one was chosen).
    let (adsb_iq_tx, _) = broadcast::channel(16);
    let adsb_device = db::get_setting::<String>(&db, adsb::DEVICE_KEY).await.ok().flatten();
    let adsb_settings = db::get_setting::<adsb::ReceiverSettings>(&db, adsb::RADIO_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let adsb_radio = radio::spawn(
        &cfg,
        radio::Role::Adsb,
        registry,
        adsb_iq_tx.clone(),
        adsb_device,
        adsb_settings.radio(),
    )?;
    let (waterfall_tx, _) = broadcast::channel(8);
    dsp::waterfall::spawn(iq_tx.subscribe(), waterfall_tx.clone(), cfg.fft_size, cfg.fps);

    // Decoders → events → SQLite, WebSocket clients and MQTT.
    let (events_tx, events_rx) = tokio::sync::mpsc::channel(1024);
    // The scheduler publishes the Doppler shift of the satellite being tracked.
    let (doppler_tx, doppler_rx) = tokio::sync::watch::channel(None);
    let rtl433 = decoders::rtl433::spawn(&db, cfg.rtl433_path.clone(), iq_tx.subscribe(), events_tx.clone()).await?;
    let aprs = decoders::aprs::spawn(
        &db,
        cfg.direwolf_path.clone(),
        iq_tx.subscribe(),
        events_tx.clone(),
        doppler_rx.clone(),
    )
    .await?;
    let (sstv_rows_tx, _) = broadcast::channel(256);
    let sstv = decoders::sstv::spawn(
        &db,
        cfg.data_dir.join("sstv"),
        iq_tx.subscribe(),
        events_tx.clone(),
        sstv_rows_tx.clone(),
        doppler_rx.clone(),
    )
    .await?;
    let recorder = recorder::spawn(iq_tx.subscribe(), doppler_rx)?;
    let schedule = scheduler::spawn(
        db.clone(),
        cfg.data_dir.clone(),
        radio.clone(),
        sstv.clone(),
        recorder,
        doppler_tx,
        events_tx,
    )
    .await?;
    let mqtt = cfg.mqtt.clone().map(|m| mqtt::spawn(m, db.clone()));
    let adsb = adsb::spawn(
        &db,
        adsb_radio,
        radio.clone(),
        adsb_iq_tx.subscribe(),
        iq_tx.subscribe(),
        schedule.clone(),
        mqtt.clone(),
    )
    .await?;
    let (event_feed, _) = broadcast::channel(256);
    tokio::spawn(events::run(db.clone(), events_rx, event_feed.clone(), mqtt.clone()));

    // Hourly cleanup of expired sessions and old events.
    {
        let db = db.clone();
        let days = cfg.event_retention_days;
        tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs(3600));
            loop {
                t.tick().await;
                match auth::purge_expired(&db).await {
                    Ok(n) if n > 0 => tracing::info!("purged {n} expired sessions"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("session purge failed: {e}"),
                }
                match events::purge(&db, days).await {
                    Ok((e, d)) if e + d > 0 => tracing::info!("purged {e} old events and {d} devices"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("event purge failed: {e}"),
                }
                if let Err(e) = adsb::purge(&db, days).await {
                    tracing::warn!("aircraft log purge failed: {e}");
                }
            }
        });
    }

    if !cfg.cookie_secure {
        tracing::warn!("SDRD_COOKIE_SECURE=false — only use this for local development over plain HTTP");
    }

    let state = web::AppState {
        db,
        cfg: Arc::new(cfg.clone()),
        radio,
        waterfall_tx,
        limiter: Arc::new(Mutex::new(auth::RateLimiter::new(Duration::from_secs(15 * 60)))),
        rtl433,
        aprs,
        sstv,
        sstv_rows_tx,
        iq_tx: iq_tx.clone(),
        audio_listeners: Default::default(),
        schedule,
        event_feed,
        mqtt,
        adsb,
    };
    let app = web::router(state);
    let listener = TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("binding {}", cfg.bind))?;
    tracing::info!(addr = %cfg.bind, source = cfg.source.name(), "sdrd listening");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("shutting down");
}

fn read_password() -> Result<String> {
    eprint!("password (min {} chars): ", auth::MIN_PASSWORD_LEN);
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let pw = line.trim_end_matches(['\r', '\n']).to_string();
    auth::validate_password(&pw)?;
    Ok(pw)
}

async fn open_db() -> Result<sqlx::SqlitePool> {
    let cfg = Config::from_env()?;
    std::fs::create_dir_all(&cfg.data_dir)?;
    db::open(&cfg.data_dir.join("sdrd.db")).await
}

async fn user_add(name: &str, role: Role) -> Result<()> {
    let db = open_db().await?;
    let pw = read_password()?;
    auth::create_user(&db, name, &pw, role).await?;
    println!("created {} user {name:?}", role.as_str());
    Ok(())
}

async fn user_passwd(name: &str) -> Result<()> {
    let db = open_db().await?;
    let pw = read_password()?;
    auth::set_password_by_name(&db, name, &pw).await?;
    println!("password updated for {name:?}; existing sessions signed out");
    Ok(())
}

/// Minimal HTTP probe for the Docker HEALTHCHECK (no curl in the image).
fn healthcheck() -> Result<()> {
    let port = std::env::var("SDRD_BIND")
        .ok()
        .and_then(|b| b.parse::<SocketAddr>().ok())
        .map(|a| a.port())
        .unwrap_or(8080);
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut buf = String::new();
    s.read_to_string(&mut buf)?;
    if buf.starts_with("HTTP/1.0 200") || buf.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        bail!("unhealthy: {}", buf.lines().next().unwrap_or(""))
    }
}

//! `divoom-export` command-line entry point.

mod cli;

use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;
use tracing_subscriber::EnvFilter;

use divoom_export::api::models::{Category, SortOrder};
use divoom_export::api::playlists::PlaylistEndpoints;
use divoom_export::api::{ClientConfig, DivoomClient};
use divoom_export::auth::{
    credentials_path, load_credentials, load_login_credentials, remove_credentials,
    save_credentials, Credentials, Session,
};
use divoom_export::error::{exit, Error};
use divoom_export::manifest::Library;
use divoom_export::playlist::select_playlist;
use divoom_export::sync::{run_sync, Selector, SyncContext, SyncOptions};
use divoom_export::{decode, export};

use cli::{AuthCommand, CatalogCommand, Cli, Command, PlaylistCommand, MAX_CATALOG_LIMIT};

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start runtime: {e}");
            return ExitCode::from(exit::GENERAL as u8);
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("error: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                eprintln!("  caused by: {s}");
                source = s.source();
            }
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

/// Build the client and log in. A fresh login happens for every network command.
async fn connect() -> Result<(DivoomClient, Session), Error> {
    let client = DivoomClient::new(ClientConfig::default())?;
    let credentials = load_credentials()?;
    let session = client.login(&credentials).await?;
    Ok((client, session))
}

fn print_json<T: Serialize>(value: &T) -> Result<(), Error> {
    let s = serde_json::to_string_pretty(value)?;
    println!("{s}");
    Ok(())
}

/// Verify credentials before replacing any previously saved account.
async fn login_and_save(
    client: &DivoomClient,
    credentials: &Credentials,
    path: &std::path::Path,
    mut output: impl std::io::Write,
) -> Result<i32, Error> {
    let session = client.login(credentials).await?;
    save_credentials(path, credentials)?;
    writeln!(
        output,
        "authenticated as user {}; credentials saved to {}",
        session.user_id,
        path.display()
    )?;
    Ok(exit::SUCCESS)
}

async fn run(cli: Cli) -> Result<i32, Error> {
    match cli.command {
        Command::Auth { command } => match command {
            AuthCommand::Check => {
                let (_, session) = connect().await?;
                println!("authenticated as user {}", session.user_id);
                Ok(exit::SUCCESS)
            }
            AuthCommand::Login => {
                let path = credentials_path()?;
                let credentials = load_login_credentials()?;
                let client = DivoomClient::new(ClientConfig::default())?;
                login_and_save(&client, &credentials, &path, std::io::stdout()).await
            }
            AuthCommand::Logout => {
                if remove_credentials(&credentials_path()?)? {
                    println!("saved credentials removed");
                } else {
                    println!("no saved credentials");
                }
                if std::env::var_os(divoom_export::auth::ENV_EMAIL).is_some()
                    || std::env::var_os(divoom_export::auth::ENV_PASSWORD_MD5).is_some()
                {
                    println!("environment credentials are still set; unset DIVOOM_EMAIL and DIVOOM_PASSWORD_MD5 in your shell to clear them");
                }
                Ok(exit::SUCCESS)
            }
        },
        Command::Playlist { command } => playlist(command).await,
        Command::Inspect { gallery_id, json } => {
            let (client, session) = connect().await?;
            let info = client.gallery_info(&session, gallery_id).await?;
            if json {
                print_json(&info)?;
            } else {
                let fmt = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_else(|| "-".into());
                println!("gallery_id:   {}", info.gallery_id.unwrap_or(gallery_id));
                println!("title:        {}", info.file_name.as_deref().unwrap_or("-"));
                println!(
                    "author:       {} (user {})",
                    info.user_name.as_deref().unwrap_or("-"),
                    info.user_id
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| "-".into())
                );
                println!("file_id:      {}", info.file_id.as_deref().unwrap_or("-"));
                println!("file_type:    {}", fmt(info.file_type));
                println!("classify:     {}", fmt(info.classify));
                println!("likes:        {}", fmt(info.like_cnt));
                println!("views:        {}", fmt(info.watch_cnt));
                println!("comments:     {}", fmt(info.comment_cnt));
                println!("private_flag: {}", fmt(info.private_flag));
                println!("hide_flag:    {}", fmt(info.hide_flag));
                println!("copyright:    {}", fmt(info.copyright_flag));
            }
            Ok(exit::SUCCESS)
        }
        Command::Fetch {
            file_id,
            output,
            raw,
        } => {
            let client = DivoomClient::new(ClientConfig::default())?;
            let (bytes, art) = divoom_export::sync::fetch_artwork(&client, &file_id).await?;
            export::write_webp_atomic(&output, &art)?;
            if let Some(raw) = &raw {
                divoom_export::manifest::write_atomic(raw, &bytes)?;
            }
            println!(
                "{} -> {} ({}x{}, {} frame(s), {} ms, format {})",
                file_id,
                output.display(),
                art.width,
                art.height,
                art.frames.len(),
                art.frame_duration_ms,
                bytes[0]
            );
            Ok(exit::SUCCESS)
        }
        Command::Decode { input, output } => {
            let bytes = std::fs::read(&input).map_err(|e| Error::io(&input, e))?;
            let art = decode::decode(&bytes)?;
            export::write_webp_atomic(&output, &art)?;
            println!(
                "{} -> {} ({}x{}, {} frame(s), {} ms)",
                input.display(),
                output.display(),
                art.width,
                art.height,
                art.frames.len(),
                art.frame_duration_ms
            );
            Ok(exit::SUCCESS)
        }
        Command::Catalog { command } => match command {
            CatalogCommand::List {
                category,
                size,
                sort,
                limit,
                json,
            } => {
                if size != 16 {
                    return Err(Error::InvalidArguments(
                        "only --size 16 is supported in version 1".into(),
                    ));
                }
                if limit == 0 || limit > MAX_CATALOG_LIMIT {
                    return Err(Error::InvalidArguments(format!(
                        "--limit must be between 1 and {MAX_CATALOG_LIMIT}"
                    )));
                }
                let category: Category = category.parse()?;
                let sort: SortOrder = sort.parse()?;
                let (client, session) = connect().await?;
                let items = client
                    .list_category(&session, category, sort, limit, 50)
                    .await?;
                if json {
                    print_json(&items)?;
                } else {
                    println!(
                        "{:>10}  {:<32}  {:<20}  {:>6}",
                        "gallery", "title", "author", "likes"
                    );
                    for it in &items {
                        println!(
                            "{:>10}  {:<32}  {:<20}  {:>6}",
                            it.gallery_id.unwrap_or(0),
                            truncate(it.file_name.as_deref().unwrap_or("-"), 32),
                            truncate(it.user_name.as_deref().unwrap_or("-"), 20),
                            it.like_cnt.unwrap_or(0)
                        );
                    }
                    println!("{} item(s)", items.len());
                }
                Ok(exit::SUCCESS)
            }
        },
    }
}

async fn playlist(command: PlaylistCommand) -> Result<i32, Error> {
    let endpoints = PlaylistEndpoints::from_env()?;
    match command {
        PlaylistCommand::Probe { json } => {
            let (client, session) = connect().await?;
            let report = client.probe_playlists(&session, &endpoints).await;
            if json {
                print_json(&report)?;
            } else {
                println!("user: {}", report.user_id);
                println!("endpoints overridden: {}", report.endpoints_overridden);
                for r in &report.results {
                    println!();
                    println!("[{}] POST {}", r.role, r.endpoint);
                    println!("  request fields: {}", r.request_schema.join(", "));
                    match &r.error {
                        Some(e) => println!("  error: {e}"),
                        None => {
                            println!("  playlists: {}", r.playlist_count);
                            println!("  private playlist visible: {}", r.private_playlist_visible);
                            for p in &r.playlists {
                                println!(
                                    "    {:>8}  {:<10}  {:<32}  items={}  first={:?}",
                                    p.id,
                                    p.visibility,
                                    truncate(&p.name, 32),
                                    p.item_count
                                        .map(|c| c.to_string())
                                        .unwrap_or_else(|| "?".into()),
                                    p.first_items_gallery_ids.as_deref().unwrap_or(&[])
                                );
                            }
                        }
                    }
                }
                println!();
                println!("items endpoint: POST {}", report.items_endpoint);
                println!(
                    "  request fields: {}",
                    report.items_request_schema.join(", ")
                );
                for n in &report.notes {
                    println!("note: {n}");
                }
            }
            Ok(exit::SUCCESS)
        }
        PlaylistCommand::List { json } => {
            let (client, session) = connect().await?;
            let playlists = client.list_owned_playlists(&session, &endpoints).await?;
            if json {
                print_json(&playlists)?;
            } else {
                println!("{:>10}  {:<10}  {:>6}  name", "id", "visibility", "items");
                for p in &playlists {
                    println!(
                        "{:>10}  {:<10}  {:>6}  {}",
                        p.id,
                        p.visibility,
                        p.item_count
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "?".into()),
                        p.name
                    );
                }
                println!("{} playlist(s)", playlists.len());
            }
            Ok(exit::SUCCESS)
        }
        PlaylistCommand::Show { selector, json } => {
            let (client, session) = connect().await?;
            let playlists = client.list_owned_playlists(&session, &endpoints).await?;
            let playlist = select_playlist(&playlists, &selector)?.clone();
            let items = client
                .list_playlist_items(&session, &endpoints, playlist.id)
                .await?;
            if json {
                #[derive(Serialize)]
                struct Shown<'a> {
                    playlist: &'a divoom_export::playlist::Playlist,
                    items: &'a [divoom_export::playlist::PlaylistEntry],
                }
                print_json(&Shown {
                    playlist: &playlist,
                    items: &items,
                })?;
            } else {
                println!(
                    "playlist {} {:?} ({}), {} item(s)",
                    playlist.id,
                    playlist.name,
                    playlist.visibility,
                    items.len()
                );
                if let Some(missing) =
                    divoom_export::playlist::missing_items(&playlist, items.len())
                {
                    eprintln!(
                        "warning: the server announces {} item(s) but listed {}; {missing} not returned by the API",
                        playlist.item_count.unwrap_or(0),
                        items.len()
                    );
                }
                println!(
                    "{:>4}  {:>10}  {:<32}  {:<20}  file_id",
                    "pos", "gallery", "title", "author"
                );
                for it in &items {
                    println!(
                        "{:>4}  {:>10}  {:<32}  {:<20}  {}",
                        it.position,
                        it.gallery_id,
                        truncate(it.title.as_deref().unwrap_or("-"), 32),
                        truncate(it.author_name.as_deref().unwrap_or("-"), 20),
                        it.file_id.as_deref().unwrap_or("-")
                    );
                }
            }
            Ok(exit::SUCCESS)
        }
        PlaylistCommand::Sync(args) => {
            let selector = if args.all {
                Selector::All
            } else {
                match args.selector {
                    Some(s) => Selector::One(s),
                    None => {
                        return Err(Error::InvalidArguments(
                            "exactly one playlist selector or --all is required".into(),
                        ))
                    }
                }
            };
            let options = SyncOptions {
                dry_run: args.dry_run,
                refresh: args.refresh,
                prune: args.prune,
                no_raw: args.no_raw,
                fail_fast: args.fail_fast,
            };
            let (client, session) = connect().await?;
            let ctx = SyncContext {
                client: &client,
                session: &session,
                endpoints: &endpoints,
                library: Library::new(&args.output),
            };
            let report = run_sync(&ctx, selector, &options).await?;
            if args.json {
                print_json(&report)?;
            } else {
                for p in &report.playlists {
                    println!(
                        "playlist {} {:?}: {} item(s), {} unique asset(s), {} failed -> {}",
                        p.playlist_id,
                        p.name,
                        p.item_count,
                        p.unique_assets,
                        p.failed_items,
                        p.manifest_path
                    );
                    if let Some(announced) = p.announced_count {
                        if announced as usize > p.item_count {
                            println!(
                                "warning: playlist {} announces {announced} item(s) but the API listed {}",
                                p.playlist_id, p.item_count
                            );
                        }
                    }
                }
                for a in report.assets.iter().filter(|a| a.error.is_some()) {
                    println!(
                        "failed {}: {}",
                        a.gallery_id,
                        a.error.as_deref().unwrap_or("")
                    );
                }
                for id in &report.removed_playlists {
                    println!("removed deleted remote playlist {id}");
                }
                println!("{}", report.summary_line());
                if !report.dry_run {
                    println!(
                        "{} link(s) in {}",
                        report.linked,
                        args.output.join(divoom_export::links::LINKS_DIR).display()
                    );
                }
            }
            Ok(if report.is_success() {
                exit::SUCCESS
            } else {
                exit::GENERAL
            })
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path as request_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn credentials() -> Credentials {
        Credentials {
            email: "test@example.com".into(),
            password_md5: "5f4dcc3b5aa765d61d8327deb882cf99".into(),
        }
    }

    #[tokio::test]
    async fn login_verifies_credentials_before_saving_them() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        let credentials = credentials();
        Mock::given(method("POST"))
            .and(request_path("/UserLogin"))
            .and(body_json(json!({
                "Email": credentials.email,
                "Password": credentials.password_md5,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ReturnCode": 0, "UserId": 123, "Token": 456,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client =
            DivoomClient::new(ClientConfig::with_hosts(&server.uri(), &server.uri())).unwrap();
        let mut output = Vec::new();
        assert_eq!(
            login_and_save(&client, &credentials, &path, &mut output)
                .await
                .unwrap(),
            exit::SUCCESS
        );
        let saved = divoom_export::auth::read_credentials(&path)
            .unwrap()
            .unwrap();
        assert_eq!(saved.email, credentials.email);
        assert_eq!(saved.password_md5, credentials.password_md5);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("authenticated as user 123"));
        assert!(!output.contains(&credentials.password_md5));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("token"));
    }

    #[tokio::test]
    async fn login_failures_preserve_saved_credentials() {
        for (status, body, exit_code) in [
            (
                200,
                json!({"ReturnCode": 1, "ReturnMessage": "invalid credentials"}),
                3,
            ),
            (200, json!({"ReturnCode": 0, "UserId": 123}), 3),
            (503, json!({}), 1),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(request_path("/UserLogin"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let mut config = ClientConfig::with_hosts(&server.uri(), &server.uri());
            config.max_attempts = 1;
            let client = DivoomClient::new(config).unwrap();
            let credentials = credentials();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials.toml");
            std::fs::write(&path, "previous credentials").unwrap();
            let mut output = Vec::new();
            let error = login_and_save(&client, &credentials, &path, &mut output)
                .await
                .unwrap_err();
            assert!(output.is_empty());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "previous credentials"
            );
            assert_eq!(error.exit_code(), exit_code);
            assert!(!error.to_string().contains(&credentials.password_md5));
            if exit_code == 3 {
                assert!(error.to_string().contains("authentication failed"));
            }
        }
    }
}

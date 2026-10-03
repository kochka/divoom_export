//! Command-line definitions (`clap` derive). No business logic lives here.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Synchronize personal Divoom playlists and export 16×16 artwork as
/// lossless animated WebP.
#[derive(Debug, Parser)]
#[command(name = "divoom-export", version, about, long_about = None)]
pub struct Cli {
    /// Increase log verbosity (-v info, -vv debug). Logs go to stderr.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Account authentication helpers.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Personal playlist operations (Profile / Playlist).
    Playlist {
        #[command(subcommand)]
        command: PlaylistCommand,
    },
    /// Inspect one artwork's metadata by gallery ID.
    Inspect {
        /// Numeric GalleryId.
        gallery_id: u64,
        /// Emit JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Download one artwork binary by CDN FileId and export it as WebP
    /// (no account needed). Useful for artworks the playlist API hides:
    /// a playlist's CoverFileId is the FileId of one of its items.
    Fetch {
        /// Relative CDN FileId, e.g. group1/M00/1B/D3/....
        file_id: String,
        /// Output `.webp` file.
        #[arg(short, long)]
        output: PathBuf,
        /// Also keep the raw binary at this path.
        #[arg(long, value_name = "FILE")]
        raw: Option<PathBuf>,
    },
    /// Decode a local Divoom binary into a WebP file (no network, no account).
    Decode {
        /// Input `.dat` file.
        input: PathBuf,
        /// Output `.webp` file.
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Bounded catalogue browser (diagnostic; not the source of truth).
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Load credentials, log in once, and print the user ID.
    Check,
    /// Verify new credentials with Divoom and save them locally.
    Login,
    /// Delete locally saved credentials (environment variables are unaffected).
    Logout,
}

#[derive(Debug, Subcommand)]
pub enum PlaylistCommand {
    /// Read-only probe of the candidate playlist endpoints.
    Probe {
        #[arg(long)]
        json: bool,
    },
    /// List playlists owned by the authenticated account.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one playlist's ordered items.
    Show {
        /// Numeric ID or exact name.
        selector: String,
        #[arg(long)]
        json: bool,
    },
    /// Synchronize one playlist or all owned playlists.
    Sync(SyncArgs),
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Numeric ID or exact name of one playlist.
    #[arg(
        value_name = "ID-OR-NAME",
        required_unless_present = "all",
        conflicts_with = "all"
    )]
    pub selector: Option<String>,
    /// Synchronize every playlist owned by the account, including private ones.
    #[arg(long)]
    pub all: bool,
    /// Destination directory.
    #[arg(long, default_value = "./library", value_name = "DIR")]
    pub output: PathBuf,
    /// Show planned operations without downloads or writes.
    #[arg(long)]
    pub dry_run: bool,
    /// Redownload and rebuild every referenced asset.
    #[arg(long)]
    pub refresh: bool,
    /// Remove unreferenced managed assets after a successful sync.
    #[arg(long)]
    pub prune: bool,
    /// Discard raw .dat files after successful WebP validation.
    #[arg(long)]
    pub no_raw: bool,
    /// Stop after the first failed asset.
    #[arg(long)]
    pub fail_fast: bool,
    /// Emit a machine-readable summary on stdout.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// List a category with a finite limit.
    List {
        /// Category name (new, character, emoji, daily, nature, symbol,
        /// pattern, creative, top, festival, recommend, plant, animal,
        /// person, food).
        #[arg(long, default_value = "top")]
        category: String,
        /// Artwork edge size; only 16 is supported.
        #[arg(long, default_value_t = 16)]
        size: u32,
        /// Sort order: new or liked.
        #[arg(long, default_value = "new")]
        sort: String,
        /// Maximum number of items (1..=500).
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
}

/// Maximum catalogue limit in version 1.
pub const MAX_CATALOG_LIMIT: usize = 500;

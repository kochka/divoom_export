# divoom-export

Standalone Rust CLI that backs up the personal playlists of a Divoom account
(**Profile / Playlist** in the Divoom app) and converts the 16×16 artworks they contain
into lossless animated WebP files, incrementally.

> **Not affiliated with, endorsed by, or supported by Divoom.** This is an independent,
> unofficial hobby project. See [Disclaimer](#disclaimer) before using it.

## Why

- The Divoom app has no export feature: what you collect in a playlist stays on your
  device and on Divoom's servers.
- Lossless WebP keeps the exact RGB pixels and millisecond frame timing of the source
  (no 256-colour palette, no 10 ms rounding as with GIF), works for static and animated
  images alike, and previews natively on a desktop.
- Your remote playlists are the desired state: you never maintain a list of IDs by hand,
  you just run `sync` again.

## Build

```bash
cargo build --release
./target/release/divoom-export --help
```

Requirements: Rust 1.85+, and `libwebp` (with `mux`/`demux`) reachable through
`pkg-config`. On Arch Linux: `pacman -S libwebp`; on Debian/Ubuntu:
`apt install libwebp-dev pkg-config`.

## Credentials

Log in once with the same email/password as the Divoom app (password input is hidden):

```bash
divoom-export auth login
divoom-export playlist list
```

`auth login` verifies the credentials with Divoom using the same authentication as
`auth check`, then saves the email and MD5 password digest in
`~/.config/divoom-export/credentials.toml` (or
`$XDG_CONFIG_HOME/divoom-export/credentials.toml` when an absolute XDG path is set):

```toml
email = "you@example.com"
password_md5 = "..."
```

The file is replaced atomically with permissions `0600` on Unix (only your user can
read or write it); newly created configuration directories use `0700`. The plaintext
password and session token are never saved. The MD5 digest is a credential equivalent:
keep this file private. Invalid credentials produce an authentication error on stderr
(exit code 3); authentication or network failures leave any previous file unchanged.

Other commands load credentials automatically: `DIVOOM_EMAIL` and
`DIVOOM_PASSWORD_MD5` override the corresponding saved values, and any missing values
are prompted for when stdin is a terminal. Complete environment credentials work
without a configuration file. `auth login` uses environment variables and prompts,
ignoring the saved file so you can replace credentials or switch accounts.

Use `divoom-export auth check` to verify the currently selected credentials without
saving anything. Use `divoom-export auth logout` to delete the saved credentials;
this works offline and does not unset environment variables in your shell.

Security properties:

- secrets are never accepted as command-line arguments (they would end up in your shell
  history and in `ps` output);
- `.env` files are not loaded automatically;
- the plaintext password, digest, session token and login payload are never logged, even
  with `-vvv`, and are redacted from error chains;
- nothing account-related is written to the library: no email, no token;
- the session token lives in memory only; every network command performs a fresh login;
- all traffic is HTTPS through `reqwest` + `rustls`.

## Commands

```bash
divoom-export auth login                            # verify and save credentials locally
divoom-export auth check                            # login and print the user id
divoom-export auth logout                           # delete saved credentials, no network
divoom-export playlist list [--json]                # your playlists (id, name, visibility, count)
divoom-export playlist show <ID-OR-NAME> [--json]   # ordered items of one playlist
divoom-export playlist sync <ID-OR-NAME> --output ./library
divoom-export playlist sync --all --output ./library [--dry-run] [--refresh] [--prune] [--no-raw] [--fail-fast] [--json]
divoom-export playlist probe [--json]               # read-only protocol diagnostic
divoom-export inspect <GALLERY_ID> [--json]         # metadata of one artwork
divoom-export fetch <FILE_ID> --output x.webp [--raw x.dat]   # one artwork by CDN FileId, no account
divoom-export decode artwork.dat --output artwork.webp        # offline, no network
divoom-export catalog list [--category top] [--size 16] [--sort new|liked] [--limit 50] [--json]
```

A playlist can be selected by numeric id or by name (exact match first, then
case-insensitive; an ambiguous name is an error).

### `playlist sync`

Exactly one playlist selector or `--all` is required. `--all` means every playlist owned
by the authenticated account, including hidden ones; it never means "every public album
on Divoom".

| Option | Effect |
|---|---|
| `--output <DIR>` | library directory, default `./library` |
| `--dry-run` | print the planned operations, no download, no write |
| `--refresh` | redownload and rebuild every referenced asset |
| `--prune` | after a fully successful sync, delete managed assets no playlist references any more |
| `--no-raw` | delete `source.dat` once the WebP is encoded and validated |
| `--fail-fast` | stop at the first failed asset instead of continuing |
| `--json` | machine-readable summary on stdout |

`--prune` is the only destructive option and is never on by default. It only touches
asset directories recorded in `state.json`, and an asset still referenced by any other
synchronized playlist is kept.

### `catalog list`

A bounded browser for Divoom's public gallery, useful to find a `GalleryId` to `inspect`.
It is a diagnostic feature, not a bulk downloader: the limit defaults to 50 and is capped
at 500. Categories: `new`, `character`, `emoji`, `daily`, `nature`, `symbol`, `pattern`,
`creative`, `top`, `festival`, `recommend`, `plant`, `animal`, `person`, `food`.

### Exit codes

`0` success · `1` general or partial failure (some assets failed, the rest were written) ·
`2` invalid arguments or ambiguous playlist selection · `3` authentication failure ·
`4` the selected playlist has nothing synchronizable.

## Output layout

```text
library/
├── state.json
├── assets/<GalleryId>-<slug>/
│   ├── artwork.webp       # lossless animated WebP
│   ├── source.dat         # original encrypted binary (kept unless --no-raw)
│   └── metadata.json      # gallery metadata as returned by the API
├── playlists/<PlaylistId>-<slug>.json
└── webp/<normalized_title>.webp -> ../assets/<GalleryId>-<slug>/artwork.webp
```

- **Assets** are deduplicated by `GalleryId` across all playlists and downloaded once.
- **Playlist manifests** are readable ordered snapshots (`position`, `gallery_id`,
  `asset_path`, `title`) that preserve remote order and duplicate entries.
- **`webp/`** is a flat directory of relative symlinks, rebuilt after each sync, so that
  every artwork referenced by a synchronized playlist is reachable under a normalized name
  such as `cute_tiger_cat.webp`. Duplicate titles get `_1`, `_2`… in ascending `GalleryId`
  order. Copy real files with `cp -L library/webp/*.webp <dest>/`. Only symlinks are ever
  removed from that directory, so you can drop your own files in it.
- **`state.json`** records every synchronized playlist and asset: `file_id`, format,
  dimensions, frame count and duration, SHA-256 of the raw and WebP files, and
  `webp_frame_count` (see below). Paths are relative, so the library can be moved. Writes
  are atomic (temporary file + rename): an interrupted sync leaves a valid library.

## Incremental behaviour

The remote `FileId` of an artwork is treated as its content identity. On every sync, for
each referenced `GalleryId`:

| Remote `FileId` | Local WebP | Local raw | Action |
|---|---|---|---|
| unchanged | valid (SHA-256 matches) | any | skip, no network |
| unchanged | missing or corrupt | valid | rebuild locally from `source.dat` |
| unchanged | missing or corrupt | missing or corrupt | redownload and rebuild |
| changed | any | any | redownload and rebuild |
| metadata unavailable | valid previous output | any | keep the previous output, record the failure |

Consequences:

- an unchanged second sync performs no CDN download;
- adding an item to a playlist downloads only that item;
- removing or reordering items rewrites the playlist manifest and state only; nothing is
  deleted without `--prune`;
- a failed update never replaces a valid output;
- state is checkpointed after each fully processed playlist;
- a single-playlist sync never treats other playlists as deleted; only `sync --all`
  reconciles playlists removed on the server side.

## Supported formats

The binaries served by Divoom's CDN are AES-128-CBC encrypted containers. Version 1
decodes the two 16×16 formats:

| Header byte | Content | Output |
|---:|---|---|
| `9` | RGB animation, frame count and frame duration in the header | animated WebP, `n × duration` timeline, infinite loop |
| `17` | LZO-compressed static RGB image | single-frame WebP |

Other values (12, 18, 26, 31, 41, 42, 43: larger or newer formats) are reported as
`unsupported format N`, the item is counted as failed and the sync continues. Decoding
never allocates from untrusted sizes without bounds and never panics on malformed input.

Pixels are exported as RGBA with alpha 255, no scaling, no colour correction. One
subtlety: libwebp merges identical consecutive frames, so a WebP may contain fewer
frames than the source (`webp_frame_count` vs `frame_count` in `state.json`). Pixels
and total duration are exact; validation reopens each file and checks dimensions and
timeline.

## Network behaviour

- one request at a time, explicit connect/read timeouts;
- at most three attempts on transient errors, HTTP 429 and 5xx, with exponential
  backoff (1 s, 2 s, 4 s) and `Retry-After` honoured;
- authentication failures and functional `ReturnCode` errors are never retried;
- downloads are streamed with a 16 MiB size limit;
- there is no unbounded mode: the catalogue browser is capped, and `sync` only touches
  your own playlists.

Please keep it that way. This tool is meant to back up a personal collection, not to
mirror Divoom's gallery.

## Protocol notes

The Divoom account API is not documented. The endpoints used here were identified by
read-only probing and are isolated in `src/api/`. Findings that may help other projects:

| Role | Endpoint | Request fields | Response |
|---|---|---|---|
| owner playlist list | `POST /Playlist/GetMyList` | `UserId, Token, StartNum, EndNum` | `PlayList[]{PlayId, Name, Describe, CoverFileId, Count, HideFlag, AddFlag}` |
| public playlist list | `POST /Playlist/GetSomeoneList` | `+ SomeOneUserId` | same shape |
| playlist items | `POST /Playlist/GetMyImageList` | `UserId, Token, PlayId, Version: 12, FileSize: 0, StartNum, EndNum` | `CurListNum, FileList[]` (gallery records) |
| public playlist items | `POST /Playlist/GetSomeoneImageList` | `+ SomeOneUserId` | `FileList[]` |

- `Token` in the login response is a JSON integer, echoed back unchanged.
- `Version` and `FileSize: 0` are required on item listings. Without `Version` the server
  answers an empty `FileList` once a playlist has more than a few items; `FileSize: 1`
  keeps only `FileType 1` records.
- Item order returned by `GetMyImageList` matches the order shown in the app.
- The playlist `Count` can exceed the number of items the API lists: some artworks are
  filtered server-side from listings and search while still resolving through
  `/Cloud/GalleryInfo` and the CDN. The tool prints a warning and reports
  `announced_count` in the JSON summary; it cannot fetch what the API does not return.
  Workaround: a playlist's `CoverFileId` (shown by `playlist list --json`) is the
  `FileId` of one of its items. Put the invisible artwork alone in a playlist, read the
  cover `FileId`, then `divoom-export fetch <FileId> --output <name>.webp`.
- `/Discover/GetAlbumList` lists Divoom's editorial albums (string `AlbumId`), not
  personal playlists.
- Unknown paths answer `ReturnCode 10 "Command is not match"`.

Endpoint paths can be overridden with `DIVOOM_ENDPOINT_OWNER_PLAYLISTS`,
`DIVOOM_ENDPOINT_PUBLIC_PLAYLISTS` and `DIVOOM_ENDPOINT_PLAYLIST_ITEMS` if Divoom changes
them. `playlist probe --json` prints a sanitized report (no token, no `FileId`) that is
useful when opening an issue.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release
```

Tests use synthetic fixtures and a mocked HTTP server (`wiremock`); none of them talks
to Divoom or needs an account.

## Disclaimer

**Use at your own risk.** This software is provided "as is", without warranty of any
kind, express or implied. In no event shall the author be liable for any claim, damages
or other liability arising from its use, including but not limited to data loss, account
suspension, or any consequence of interacting with third-party services. See
[LICENSE](LICENSE).

- This project is **not affiliated with, endorsed by, sponsored by, or supported by
  Divoom** or any of its subsidiaries. "Divoom", "Pixoo" and related names are
  trademarks of their respective owners and are used here for identification only.
- It relies on an **undocumented, private API** that may change or stop working at any
  time without notice. Using it may be contrary to Divoom's terms of service; you are
  solely responsible for checking those terms and for the consequences of using this
  tool with your account.
- It is intended **exclusively for personal backup** of the playlists you curated in
  your own account. The artworks it downloads are created by Divoom community members
  and remain **the property of their respective authors**. This tool grants no right
  whatsoever on that content: do not redistribute, publish, sell or otherwise reuse
  exported artworks without the author's permission. Respect the copyright and
  visibility settings chosen by creators and by Divoom.
- The tool contains no artwork, no credentials and no account data. Its decoding logic
  is based on formats already documented by other open-source projects
  ([apixoo](https://github.com/redphx/apixoo),
  [DivoomClient](https://github.com/redphx/DivoomClient)); it does not remove any
  access control, and it requires you to log in with your own account for anything
  that is not already publicly served.
- Do not use it to scrape, mirror or bulk-download Divoom's gallery, to overload their
  servers, or to access content you are not authorized to view.

If you are the copyright holder of content or code you believe is misused by this
project, open an issue and it will be addressed promptly.

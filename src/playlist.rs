//! Playlist domain model and selection helpers.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Visibility of a remote playlist as reported by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
    /// The API did not report a visibility flag.
    Unknown,
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Unknown => "unknown",
        })
    }
}

/// A remote personal playlist (protocol name: album).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub visibility: Visibility,
    #[serde(default)]
    pub item_count: Option<u64>,
    #[serde(default)]
    pub cover_file_id: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// One ordered entry of a playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub position: usize,
    pub gallery_id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub author_user_id: Option<u64>,
    #[serde(default)]
    pub file_id: Option<String>,
}

/// Number of items the server announced but did not list, if any.
pub fn missing_items(playlist: &Playlist, listed: usize) -> Option<u64> {
    let announced = playlist.item_count?;
    let listed = listed as u64;
    announced.checked_sub(listed).filter(|m| *m > 0)
}

/// Select one playlist by numeric ID or exact name.
///
/// Numeric selection is unambiguous. A name must match exactly one playlist;
/// zero or several matches return [`Error::PlaylistSelection`] listing
/// candidate IDs.
pub fn select_playlist<'a>(playlists: &'a [Playlist], selector: &str) -> Result<&'a Playlist> {
    let selector = selector.trim();
    if let Ok(id) = selector.parse::<u64>() {
        if let Some(p) = playlists.iter().find(|p| p.id == id) {
            return Ok(p);
        }
        return Err(Error::PlaylistSelection(format!(
            "no playlist with ID {id}; available: {}",
            describe(playlists)
        )));
    }
    let matches: Vec<&Playlist> = playlists.iter().filter(|p| p.name == selector).collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => {
            let ci: Vec<&Playlist> = playlists
                .iter()
                .filter(|p| p.name.eq_ignore_ascii_case(selector))
                .collect();
            if ci.len() == 1 {
                return Ok(ci[0]);
            }
            Err(Error::PlaylistSelection(format!(
                "no playlist named {selector:?}; available: {}",
                describe(playlists)
            )))
        }
        _ => Err(Error::PlaylistSelection(format!(
            "playlist name {selector:?} is ambiguous; candidates: {}",
            describe(&matches.iter().map(|p| (*p).clone()).collect::<Vec<_>>())
        ))),
    }
}

fn describe(playlists: &[Playlist]) -> String {
    if playlists.is_empty() {
        return "(none)".into();
    }
    playlists
        .iter()
        .map(|p| format!("{} ({:?})", p.id, p.name))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(id: u64, name: &str) -> Playlist {
        Playlist {
            id,
            name: name.into(),
            visibility: Visibility::Public,
            item_count: None,
            cover_file_id: None,
            updated_at: None,
        }
    }

    #[test]
    fn missing_items_is_only_reported_when_count_exceeds_listing() {
        let mut p = pl(1, "A");
        assert_eq!(missing_items(&p, 5), None);
        p.item_count = Some(30);
        assert_eq!(missing_items(&p, 27), Some(3));
        assert_eq!(missing_items(&p, 30), None);
        assert_eq!(missing_items(&p, 31), None);
    }

    #[test]
    fn selects_by_id_and_name() {
        let list = vec![pl(1, "A"), pl(2, "B"), pl(3, "B")];
        assert_eq!(select_playlist(&list, "1").unwrap().id, 1);
        assert_eq!(select_playlist(&list, "A").unwrap().id, 1);
        assert_eq!(select_playlist(&list, "a").unwrap().id, 1);
        let err = select_playlist(&list, "B").unwrap_err();
        assert!(matches!(err, Error::PlaylistSelection(_)));
        assert!(err.to_string().contains("2") && err.to_string().contains("3"));
        assert!(matches!(
            select_playlist(&list, "Z"),
            Err(Error::PlaylistSelection(_))
        ));
        assert!(matches!(
            select_playlist(&list, "9"),
            Err(Error::PlaylistSelection(_))
        ));
    }
}

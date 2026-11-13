//! The GitHub "latest release" JSON and the `SHA256SUMS.txt` that release.yml publishes with it.

use super::version::Version;

/// The checksum file attached to every release.
pub const CHECKSUMS: &str = "SHA256SUMS.txt";

/// The installer file name for `v`, e.g. "Setup-VoiceChanger-1.1.0.exe".
pub fn installer_name(v: Version) -> String {
    format!("Setup-VoiceChanger-{v}.exe")
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub title: String,
    pub notes: String,
    pub date: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    pub title: String,
    pub notes: String,
    pub date: String,
    pub installer: Asset,
    pub checksums: Asset,
}

/// Err for invalid JSON / no tag_name. Ok(None) for draft, prerelease, or a tag that is not strict vX.Y.Z.
/// title = name, or "VoiceChanger X.Y.Z" if null/empty; notes = body with \r\n → \n ("" if null);
/// date = first 10 chars of published_at ("" if null).
pub fn parse_release(json: &str) -> Result<Option<Release>, String> {
    let r: ReleaseJson =
        serde_json::from_str(json).map_err(|e| format!("invalid release JSON: {e}"))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::parse_tag(&r.tag_name) else {
        return Ok(None);
    };
    let title = r
        .name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("VoiceChanger {version}"));
    let notes = r.body.unwrap_or_default().replace("\r\n", "\n");
    let date = r
        .published_at
        .map(|d| d.chars().take(10).collect())
        .unwrap_or_default();
    Ok(Some(Release {
        version,
        title,
        notes,
        date,
        assets: r.assets,
    }))
}

/// The fields of GitHub's release object that the updater reads; the rest are ignored.
#[derive(serde::Deserialize)]
struct ReleaseJson {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

impl Release {
    /// Exact names. Err("the release has no Setup-VoiceChanger-1.1.0.exe") / Err("the release has no SHA256SUMS.txt").
    pub fn into_update(self) -> Result<Update, String> {
        let installer = installer_name(self.version);
        let find = |name: &str| {
            self.assets
                .iter()
                .find(|a| a.name == name)
                .cloned()
                .ok_or_else(|| format!("the release has no {name}"))
        };
        let installer = find(&installer)?;
        let checksums = find(CHECKSUMS)?;
        Ok(Update {
            version: self.version,
            title: self.title,
            notes: self.notes,
            date: self.date,
            installer,
            checksums,
        })
    }
}

/// Lowercase hex for `file`: lines `<64 hex><ws><name>` or `<hex> *<name>`; hex any case; others ignored.
pub fn parse_sums(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (hex, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name).trim_end();
        let valid = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit());
        (valid && name == file).then(|| hex.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST: &str = r####"{
        "url": "https://api.github.com/repos/mmhy2003/VoiceChanger/releases/1",
        "html_url": "https://github.com/mmhy2003/VoiceChanger/releases/tag/v1.1.0",
        "id": 1,
        "author": { "login": "mmhy2003", "id": 2 },
        "tag_name": "v1.1.0",
        "name": "VoiceChanger 1.1.0",
        "draft": false,
        "prerelease": false,
        "created_at": "2026-12-01T09:00:00Z",
        "published_at": "2026-12-01T10:00:00Z",
        "body": "### Added\r\n- Updates",
        "assets": [
            {
                "id": 10,
                "name": "Setup-VoiceChanger-1.1.0.exe",
                "content_type": "application/octet-stream",
                "size": 123456789,
                "browser_download_url": "https://github.com/mmhy2003/VoiceChanger/releases/download/v1.1.0/Setup-VoiceChanger-1.1.0.exe"
            },
            {
                "id": 11,
                "name": "SHA256SUMS.txt",
                "content_type": "text/plain",
                "size": 95,
                "browser_download_url": "https://github.com/mmhy2003/VoiceChanger/releases/download/v1.1.0/SHA256SUMS.txt"
            }
        ]
    }"####;

    const V110: Version = Version {
        major: 1,
        minor: 1,
        patch: 0,
    };

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            browser_download_url: format!("https://example.invalid/{name}"),
            size: 1,
        }
    }

    fn release(assets: Vec<Asset>) -> Release {
        Release {
            version: V110,
            title: "VoiceChanger 1.1.0".to_owned(),
            notes: "notes".to_owned(),
            date: "2026-12-01".to_owned(),
            assets,
        }
    }

    #[test]
    fn parses_latest_fixture() {
        let r = parse_release(LATEST).unwrap().unwrap();
        assert_eq!(r.version, V110);
        assert_eq!(r.title, "VoiceChanger 1.1.0");
        assert_eq!(r.notes, "### Added\n- Updates");
        assert_eq!(r.date, "2026-12-01");
        assert_eq!(r.assets.len(), 2);
        assert_eq!(
            r.assets[0],
            Asset {
                name: "Setup-VoiceChanger-1.1.0.exe".to_owned(),
                browser_download_url: "https://github.com/mmhy2003/VoiceChanger/releases/download/v1.1.0/Setup-VoiceChanger-1.1.0.exe".to_owned(),
                size: 123456789,
            }
        );
        assert_eq!(r.assets[1].name, CHECKSUMS);
    }

    #[test]
    fn null_name_and_body() {
        let json = LATEST
            .replace(r#""VoiceChanger 1.1.0""#, "null")
            .replace(r####""### Added\r\n- Updates""####, "null");
        let r = parse_release(&json).unwrap().unwrap();
        assert_eq!(r.title, "VoiceChanger 1.1.0");
        assert_eq!(r.notes, "");
        assert_eq!(r.date, "2026-12-01");

        let empty_name = LATEST.replace(r#""VoiceChanger 1.1.0""#, r#""""#);
        let r = parse_release(&empty_name).unwrap().unwrap();
        assert_eq!(r.title, "VoiceChanger 1.1.0");

        let no_date = LATEST.replace(r#""published_at": "2026-12-01T10:00:00Z","#, "");
        let r = parse_release(&no_date).unwrap().unwrap();
        assert_eq!(r.date, "");
    }

    #[test]
    fn skips_drafts_prereleases_and_odd_tags() {
        let draft = LATEST.replace(r#""draft": false"#, r#""draft": true"#);
        let pre = LATEST.replace(r#""prerelease": false"#, r#""prerelease": true"#);
        let base = LATEST.replace(r#""tag_name": "v1.1.0""#, r#""tag_name": "base-models-1""#);
        let rc = LATEST.replace(r#""tag_name": "v1.1.0""#, r#""tag_name": "v1.1.0-rc1""#);
        for json in [draft, pre, base, rc] {
            assert_eq!(parse_release(&json), Ok(None), "{json}");
        }
    }

    #[test]
    fn bad_json_is_error() {
        for json in ["not json", "{}"] {
            let err = parse_release(json).unwrap_err();
            assert!(err.starts_with("invalid release JSON: "), "{err}");
        }
    }

    #[test]
    fn into_update_picks_assets() {
        let r = release(vec![
            asset("Setup-VoiceChanger-1.0.0.exe"),
            asset(CHECKSUMS),
            asset("Setup-VoiceChanger-1.1.0.exe"),
        ]);
        let u = r.into_update().unwrap();
        assert_eq!(u.version, V110);
        assert_eq!(u.title, "VoiceChanger 1.1.0");
        assert_eq!(u.notes, "notes");
        assert_eq!(u.date, "2026-12-01");
        assert_eq!(u.installer, asset("Setup-VoiceChanger-1.1.0.exe"));
        assert_eq!(u.checksums, asset(CHECKSUMS));
    }

    #[test]
    fn into_update_missing_assets() {
        let no_installer = release(vec![
            asset("setup-voicechanger-1.1.0.exe"),
            asset(CHECKSUMS),
        ]);
        assert_eq!(
            no_installer.into_update(),
            Err("the release has no Setup-VoiceChanger-1.1.0.exe".to_owned())
        );
        let no_sums = release(vec![asset("Setup-VoiceChanger-1.1.0.exe")]);
        assert_eq!(
            no_sums.into_update(),
            Err("the release has no SHA256SUMS.txt".to_owned())
        );
    }

    #[test]
    fn parse_sums_formats() {
        let hex = "0123456789abcdef".repeat(4);
        let name = "Setup-VoiceChanger-1.1.0.exe";
        let want = Some(hex.clone());

        assert_eq!(parse_sums(&format!("{hex}  {name}\n"), name), want);
        assert_eq!(
            parse_sums(&format!("{}  {name}\n", hex.to_uppercase()), name),
            want
        );
        assert_eq!(parse_sums(&format!("{hex} *{name}\n"), name), want);
        assert_eq!(
            parse_sums(
                &format!("{}  other.zip\r\n{hex}  {name}\r\n", "f".repeat(64)),
                name
            ),
            want
        );
        assert_eq!(parse_sums(&format!("{hex}  other.zip\n"), name), None);
        assert_eq!(parse_sums(&format!("{}  {name}\n", &hex[1..]), name), None);
        assert_eq!(parse_sums(&format!("{}g  {name}\n", &hex[1..]), name), None);
        assert_eq!(parse_sums("", name), None);
    }

    #[test]
    fn installer_name_format() {
        assert_eq!(installer_name(V110), "Setup-VoiceChanger-1.1.0.exe");
        assert_eq!(
            installer_name(Version {
                major: 10,
                minor: 0,
                patch: 23
            }),
            "Setup-VoiceChanger-10.0.23.exe"
        );
    }
}

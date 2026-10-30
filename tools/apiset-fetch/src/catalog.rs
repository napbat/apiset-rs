//! Find update packages in the Microsoft Update Catalog.
//!
//! The catalog (<https://www.catalog.update.microsoft.com>) has a search page and a download
//! dialog. The search page lists the updates for a KB number, one row for each product and
//! architecture. The download dialog gives the package URLs of one update, also the URLs of
//! its prerequisite packages.

use crate::Result;

/// The catalog search page.
const SEARCH_URL: &str = "https://www.catalog.update.microsoft.com/Search.aspx";
/// The catalog download dialog.
const DOWNLOAD_URL: &str = "https://www.catalog.update.microsoft.com/DownloadDialog.aspx";

/// One row of a catalog search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogUpdate {
    /// The update ID (a GUID).
    pub id: String,
    /// The title, for example `2023-03 Cumulative Update Preview for Windows 11 Version 22H2
    /// for x64-based Systems (KB5023778)`.
    pub title: String,
}

/// Search the catalog for a KB number.
///
/// # Errors
///
/// Returns an error if the request fails.
pub fn search(agent: &ureq::Agent, kb: &str) -> Result<Vec<CatalogUpdate>> {
    let page = agent
        .get(SEARCH_URL)
        .query("q", kb)
        .call()?
        .body_mut()
        .read_to_string()?;
    Ok(parse_search(&page))
}

/// Return the package URLs of an update.
///
/// # Errors
///
/// Returns an error if the request fails.
pub fn download_urls(agent: &ureq::Agent, id: &str) -> Result<Vec<String>> {
    let ids = format!(r#"[{{"size":0,"languages":"","uidInfo":"{id}","updateID":"{id}"}}]"#);
    let page = agent
        .post(DOWNLOAD_URL)
        .send_form([("updateIDs", ids.as_str())])?
        .body_mut()
        .read_to_string()?;
    Ok(parse_download_dialog(&page))
}

/// Parse the rows of a search page. Each row has a link with the ID `<guid>_link`.
pub fn parse_search(page: &str) -> Vec<CatalogUpdate> {
    let mut updates = Vec::new();
    let mut rest = page;
    while let Some(start) = rest.find("_link") {
        let before = &rest[..start];
        let id_start = before.rfind(['"', '\'']).map_or(0, |i| i + 1);
        let id = &before[id_start..];
        let after = &rest[start..];
        let title = after
            .find('>')
            .and_then(|open| {
                let text = &after[open + 1..];
                text.find("</a>").map(|close| &text[..close])
            })
            .unwrap_or_default();
        if is_guid(id) {
            updates.push(CatalogUpdate {
                id: id.to_owned(),
                title: decode_entities(&title.split_whitespace().collect::<Vec<_>>().join(" ")),
            });
        }
        rest = &rest[start + "_link".len()..];
    }
    updates
}

/// Parse the package URLs of a download dialog: the values of `.url = '...'`.
pub fn parse_download_dialog(page: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = page;
    while let Some(start) = rest.find(".url") {
        rest = &rest[start + ".url".len()..];
        let Some(open) = rest.find('\'') else { break };
        let value = &rest[open + 1..];
        let Some(close) = value.find('\'') else { break };
        let prefix = rest[..open].trim();
        if prefix == "=" {
            urls.push(value[..close].to_owned());
        }
        rest = &value[close + 1..];
    }
    urls
}

/// Return `true` if `text` has the form of a GUID.
fn is_guid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Decode the HTML entities that catalog titles use.
fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Return the words that a catalog title has for a Winbindex release, for example
/// `["Windows 11", "24H2"]` for `11-24H2` and `["Windows 10", "1607"]` for `1607`.
pub fn release_words(release: &str) -> [String; 2] {
    match release.strip_prefix("11-") {
        Some(version) => ["Windows 11".to_owned(), version.to_owned()],
        None => ["Windows 10".to_owned(), release.to_owned()],
    }
}

/// Pick the x64 client update of a release from the search rows.
pub fn pick<'a>(updates: &'a [CatalogUpdate], release: &str) -> Option<&'a CatalogUpdate> {
    let words = release_words(release);
    updates.iter().find(|update| {
        let title = &update.title;
        title.contains("x64-based")
            && !title.contains("Server")
            && !title.contains("Dynamic")
            && words.iter().all(|word| title.contains(word.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"
        <a id='c671b1df-825e-4bf9-9da3-ce9141290899_link' href="javascript:void(0);">
            2026-09 Cumulative Update Preview for Windows 11, version 24H2 for x64-based
            Systems (KB5124010) (26100.9550)
        </a>
        <a id='cfb74803-97f6-4da5-977b-0e07b766d2bb_link'>
            2026-09 Cumulative Update Preview for Windows 11, version 24H2 for arm64-based Systems (KB5124010)
        </a>
        <a id='not-a-guid_link'>x</a>"#;

    const DIALOG: &str = "downloadInformation[0].files[0].url = 'https://example/a-x64.msu';\n\
        downloadInformation[0].files[1].url = 'https://example/b-x64.msu';\n\
        downloadInformation[0].files[0].digest = 'x';";

    #[test]
    fn search_rows_and_download_urls_parse() {
        let rows = parse_search(SEARCH);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "c671b1df-825e-4bf9-9da3-ce9141290899");
        assert_eq!(
            rows[0].title,
            "2026-09 Cumulative Update Preview for Windows 11, version 24H2 for x64-based Systems (KB5124010) (26100.9550)"
        );
        assert_eq!(
            pick(&rows, "11-24H2").map(|r| r.id.as_str()),
            Some(rows[0].id.as_str())
        );
        assert_eq!(pick(&rows, "11-23H2"), None);
        assert_eq!(
            parse_download_dialog(DIALOG),
            ["https://example/a-x64.msu", "https://example/b-x64.msu"]
        );
    }
}

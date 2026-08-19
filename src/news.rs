use serde::Deserialize;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct NewsItem {
    pub title: String,
    pub body: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
struct PatchNotes {
    #[serde(default)]
    entries: Vec<PatchEntry>,
}

#[derive(Debug, Deserialize)]
struct PatchEntry {
    title: Option<String>,
    #[serde(rename = "version", default)]
    version: Option<String>,
    body: Option<String>,
    content: Option<PatchContent>,
}

#[derive(Debug, Deserialize)]
struct PatchContent {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

pub async fn fetch(client: &reqwest::Client) -> Result<Vec<NewsItem>> {
    let notes: PatchNotes = client
        .get(crate::mojang::NEWS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut items = Vec::new();
    for entry in notes.entries.into_iter().take(20) {
        let title = entry
            .title
            .or_else(|| entry.content.as_ref().and_then(|c| c.title.clone()))
            .unwrap_or_else(|| "Minecraft".into());
        let body = entry
            .body
            .or_else(|| entry.content.as_ref().and_then(|c| c.body.clone()))
            .unwrap_or_default();
        items.push(NewsItem {
            title: strip_html(&title),
            body: strip_html(&body),
            version: entry.version.unwrap_or_default(),
        });
    }
    Ok(items)
}

fn strip_html(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 280 {
        let cut: String = collapsed.chars().take(277).collect();
        format!("{cut}…")
    } else {
        collapsed
    }
}

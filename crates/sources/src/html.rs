//! Just enough HTML for the pages of a site like khinsider: the rows of a
//! table (cells with their text and first link) and the links of a page.
//! Tolerant of the markup those pages have (unclosed cells, attributes in
//! any order, entities).

/// One table cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub href: Option<String>,
    /// A header cell (`<th>`).
    pub head: bool,
    /// The columns it covers (`colspan`, 1 without one).
    pub span: usize,
}

/// The table opened by the first tag containing `marker` (`id="songlist"`),
/// as rows of cells; empty when there is none.
pub fn table(html: &str, marker: &str) -> Vec<Vec<Cell>> {
    let Some(at) = html.find(marker) else { return Vec::new() };
    let start = html[..at].rfind("<table").unwrap_or(at);
    let end = html[start..].find("</table").map_or(html.len(), |e| start + e);
    let body = &html[start..end];
    let mut rows = Vec::new();
    for tr in body.split("<tr").skip(1) {
        let tr = tr.split("</tr").next().unwrap_or(tr);
        let mut cells = Vec::new();
        let mut rest = tr;
        loop {
            let td = rest.find("<td");
            let th = rest.find("<th");
            let (pos, head) = match (td, th) {
                (Some(a), Some(b)) if b < a => (b, true),
                (Some(a), _) => (a, false),
                (None, Some(b)) => (b, true),
                (None, None) => break,
            };
            let after = &rest[pos + 3..];
            let Some(open_end) = after.find('>') else { break };
            let span = attr(&format!(" {}", &after[..open_end]), "colspan")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let inner_all = &after[open_end + 1..];
            let close = ["</td", "</th", "<td", "<th"]
                .iter()
                .filter_map(|m| inner_all.find(m))
                .min()
                .unwrap_or(inner_all.len());
            let inner = &inner_all[..close];
            cells.push(Cell {
                text: text_of(inner),
                href: first_href(inner),
                head,
                span,
            });
            rest = &inner_all[close..];
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    rows
}

/// The `href` of the first link in `html`, entities decoded.
pub fn first_href(html: &str) -> Option<String> {
    links(html).into_iter().next().map(|(h, _)| h)
}

/// Every link of `html`: its `href` and its text.
pub fn links(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<a ") {
        let tag_end = rest[pos..].find('>').map_or(rest.len(), |e| pos + e);
        let tag = &rest[pos..tag_end];
        let close = rest[tag_end..].find("</a>").map_or(rest.len(), |e| tag_end + e);
        if let Some(h) = attr(tag, "href") {
            let text = text_of(rest.get(tag_end + 1..close).unwrap_or_default());
            out.push((h, text));
        }
        rest = &rest[(tag_end + 1).min(rest.len())..];
    }
    out
}

/// An attribute's value in a tag (quoted with `"` or `'`).
pub fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before = lower[..at].chars().next_back();
        let rest = lower[at + name.len()..].trim_start();
        if before.is_some_and(char::is_whitespace) && rest.starts_with('=') {
            let raw = &tag[tag.len() - rest.len() + 1..];
            let raw = raw.trim_start();
            let q = raw.chars().next()?;
            let value = if q == '"' || q == '\'' {
                let v = &raw[1..];
                &v[..v.find(q)?]
            } else {
                raw.split(|c: char| c.is_whitespace() || c == '>').next()?
            };
            return Some(decode(value));
        }
        from = at + name.len();
    }
    None
}

/// The text of a piece of HTML: tags dropped, entities decoded, spaces
/// collapsed.
pub fn text_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// HTML entities: the named ones those pages use and numeric ones.
pub fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cells_links_and_entities() {
        let html = r#"<p>x</p><table id="songlist"><tr id="h"><th>CD</th><th colspan="2">Song Name</th><th>FLAC</th></tr>
            <tr><td align="center">1</td><td class="clickable-row"><a href="/a/01.%2520Rock &amp; Roll.mp3">Rock &amp; Roll&#39;s</a></td><td>22 MB</td></tr>
            <tr><td>2<td><a href='/b'>B</a></tr></table><table><tr><td>other</td></tr></table>"#;
        let rows = table(html, r#"id="songlist""#);
        assert_eq!(rows.len(), 3);
        assert!(rows[0][0].head);
        assert_eq!(rows[0][1].span, 2);
        assert_eq!(rows[1][1].text, "Rock & Roll's");
        assert_eq!(rows[1][1].href.as_deref(), Some("/a/01.%2520Rock & Roll.mp3"));
        assert_eq!(rows[2][1].href.as_deref(), Some("/b"), "unclosed cells");
        assert!(table(html, "nothing").is_empty());
        assert_eq!(attr(r#"<a class="x" href = "y">"#, "href").as_deref(), Some("y"));
        assert_eq!(attr(r#"<a data-href="z" href="y">"#, "href").as_deref(), Some("y"));
    }
}

use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: Option<String>,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThemeTokens {
    id: String,
    name: String,
    mode: String,
    paper: String,
    ink: String,
    muted: String,
    accent: String,
    rule: String,
    code_bg: String,
    font_body: String,
    font_heading: String,
    base_size: f32,
    line_height: f32,
    measure: f32,
    paragraph_gap: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportOptions {
    toc: bool,
    page_numbers: bool,
    header_left: String,
    header_center: String,
    header_right: String,
    footer_left: String,
    footer_center: String,
    footer_right: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: Option<i64>,
    name: String,
    files: Vec<StudioFile>,
    theme: ThemeTokens,
    font_size: f32,
    typeface: String,
    export_options: ExportOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportRequest {
    id: Option<i64>,
    name: String,
    files: Vec<StudioFile>,
    theme: ThemeTokens,
    font_size: f32,
    typeface: String,
    export_options: ExportOptions,
    format: String,
    output_path: String,
}

#[derive(Serialize)]
struct ProjectSummary {
    id: i64,
    name: String,
    updated_at: String,
    file_count: usize,
}

fn database(app: &AppHandle) -> Result<Connection, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let connection = Connection::open(directory.join("markdown-studio.sqlite3"))
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS projects (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               name TEXT NOT NULL,
               payload TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               file_count INTEGER NOT NULL DEFAULT 0
             );",
        )
        .map_err(|error| error.to_string())?;
    let has_file_count = {
        let mut statement = connection
            .prepare("PRAGMA table_info(projects)")
            .map_err(|error| error.to_string())?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| error.to_string())?;
        let mut found = false;
        for column in columns {
            if column.map_err(|error| error.to_string())? == "file_count" {
                found = true;
                break;
            }
        }
        found
    };
    if !has_file_count {
        connection
            .execute_batch(
                "ALTER TABLE projects ADD COLUMN file_count INTEGER NOT NULL DEFAULT 0;
                 UPDATE projects
                 SET file_count = CASE
                   WHEN json_valid(payload) THEN json_array_length(payload, '$.files')
                   ELSE 0
                 END;",
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(connection)
}

#[tauri::command]
async fn render_markdown(markdown: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || markdown_html(&markdown))
        .await
        .map_err(|error| error.to_string())
}

fn markdown_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(markdown, options);
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

#[tauri::command]
async fn read_markdown_paths(paths: Vec<String>) -> Result<Vec<StudioFile>, String> {
    tauri::async_runtime::spawn_blocking(move || read_markdown_paths_blocking(paths))
        .await
        .map_err(|error| error.to_string())?
}

fn read_markdown_paths_blocking(paths: Vec<String>) -> Result<Vec<StudioFile>, String> {
    let mut markdown_paths = Vec::new();
    for path in paths {
        collect_markdown(Path::new(&path), &mut markdown_paths)?;
    }
    markdown_paths.sort_by(|left, right| natural_key(left).cmp(&natural_key(right)));
    markdown_paths
        .into_iter()
        .map(|path| {
            let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let name = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Untitled")
                .to_string();
            Ok(StudioFile {
                name,
                path: Some(path.to_string_lossy().into_owned()),
                content,
            })
        })
        .collect()
}

fn collect_markdown(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            collect_markdown(&entry.map_err(|error| error.to_string())?.path(), output)?;
        }
    } else if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
    {
        output.push(path.to_owned());
    }
    Ok(())
}

fn natural_key(path: &Path) -> (u64, String) {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let digits: String = name
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect();
    (
        digits.parse().unwrap_or(u64::MAX),
        name.to_ascii_lowercase(),
    )
}

#[tauri::command]
async fn save_project(app: AppHandle, project: Project) -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(move || save_project_blocking(app, project))
        .await
        .map_err(|error| error.to_string())?
}

fn save_project_blocking(app: AppHandle, mut project: Project) -> Result<i64, String> {
    let connection = database(&app)?;
    let now = chrono::Utc::now().to_rfc3339();
    let file_count = project.files.len() as i64;
    if let Some(id) = project.id {
        let payload = serde_json::to_string(&project).map_err(|error| error.to_string())?;
        connection
            .execute(
                "UPDATE projects SET name = ?1, payload = ?2, updated_at = ?3, file_count = ?4 WHERE id = ?5",
                params![project.name, payload, now, file_count, id],
            )
            .map_err(|error| error.to_string())?;
        Ok(id)
    } else {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO projects (name, payload, updated_at, file_count) VALUES (?1, '', ?2, ?3)",
                params![project.name, now, file_count],
            )
            .map_err(|error| error.to_string())?;
        let id = transaction.last_insert_rowid();
        project.id = Some(id);
        let payload = serde_json::to_string(&project).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE projects SET payload = ?1 WHERE id = ?2",
                params![payload, id],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(id)
    }
}

#[tauri::command]
async fn list_projects(app: AppHandle) -> Result<Vec<ProjectSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || list_projects_blocking(app))
        .await
        .map_err(|error| error.to_string())?
}

fn list_projects_blocking(app: AppHandle) -> Result<Vec<ProjectSummary>, String> {
    let connection = database(&app)?;
    let mut statement = connection
        .prepare("SELECT id, name, updated_at, file_count FROM projects ORDER BY updated_at DESC")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let (id, name, updated_at, file_count) = row.map_err(|error| error.to_string())?;
        Ok(ProjectSummary {
            id,
            name,
            updated_at,
            file_count: file_count.max(0) as usize,
        })
    })
    .collect()
}

#[tauri::command]
async fn load_project(app: AppHandle, id: i64) -> Result<Project, String> {
    tauri::async_runtime::spawn_blocking(move || load_project_blocking(app, id))
        .await
        .map_err(|error| error.to_string())?
}

fn load_project_blocking(app: AppHandle, id: i64) -> Result<Project, String> {
    let connection = database(&app)?;
    let payload: String = connection
        .query_row("SELECT payload FROM projects WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&payload).map_err(|error| error.to_string())
}

#[tauri::command]
async fn export_document(request: ExportRequest) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || export_document_blocking(request))
        .await
        .map_err(|error| error.to_string())?
}

fn export_document_blocking(request: ExportRequest) -> Result<(), String> {
    let _ = request.id;
    let path = Path::new(&request.output_path);
    match request.format.as_str() {
        "html" => fs::write(path, build_html(&request)).map_err(|error| error.to_string()),
        "md" => fs::write(path, build_markdown(&request.files)).map_err(|error| error.to_string()),
        "docx" => write_docx(path, &request),
        "pdf" => write_pdf(path, &request),
        format => Err(format!("Unsupported export format: {format}")),
    }
}

fn build_markdown(files: &[StudioFile]) -> String {
    let mut output = String::new();
    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            output.push_str("\n\n");
        }
        output.push_str(
            file.content
                .trim_end_matches(|character| character == '\r' || character == '\n'),
        );
    }
    output.push('\n');
    output
}

fn build_html(request: &ExportRequest) -> String {
    let title = escape_html(&request.name);
    let mut toc = String::new();
    let mut body = String::new();
    for (file_index, file) in request.files.iter().enumerate() {
        let section_id = format!("section-{file_index}");
        toc.push_str(&format!(
            "<li><a href=\"#{section_id}\">{}</a></li>",
            escape_html(&file.name)
        ));
        body.push_str(&format!(
            "<section id=\"{section_id}\" class=\"source-file\"><div class=\"source-label\">{:02} · {}</div>{}</section>",
            file_index + 1,
            escape_html(&file.name),
            markdown_html(&file.content)
        ));
    }
    let toc_page = if request.export_options.toc {
        format!("<nav class=\"toc\"><h1>Contents</h1><ol>{toc}</ol></nav>")
    } else {
        String::new()
    };
    let theme = &request.theme;
    let typeface = match request.typeface.as_str() {
        "mono" => "ui-monospace, SFMono-Regular, Consolas, monospace",
        "sans" => "ui-sans-serif, system-ui, sans-serif",
        _ => "Iowan Old Style, Palatino Linotype, Georgia, serif",
    };
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>{title}</title>
<style>
:root{{--paper:{paper};--ink:{ink};--muted:{muted};--accent:{accent};--rule:{rule};--code:{code};}}
*{{box-sizing:border-box}} html{{background:var(--paper);color:var(--ink)}} body{{margin:0;font:{size}px/{leading} {typeface}}}
main{{max-width:{measure}px;margin:auto;padding:72px 36px}} h1,h2,h3,h4{{font-family:ui-sans-serif,system-ui,sans-serif;line-height:1.15;letter-spacing:-.025em}}
h1{{font-size:2.4em;border-bottom:1px solid var(--rule);padding-bottom:.35em}} h2{{margin-top:2em}} p{{margin:0 0 {gap}em}}
a{{color:var(--accent)}} blockquote{{border-left:3px solid var(--accent);padding-left:1.2em;color:var(--muted)}} code,pre{{font-family:ui-monospace,monospace;background:var(--code)}} code{{padding:.15em .3em;border-radius:4px}} pre{{padding:1.2em;overflow:auto;border-radius:8px}}
table{{border-collapse:collapse;width:100%}} td,th{{border-bottom:1px solid var(--rule);padding:.55em;text-align:left}}
.source-file{{break-before:page;min-height:80vh}}.source-file:first-of-type{{break-before:auto}}.source-label{{font:10px ui-sans-serif;text-transform:uppercase;letter-spacing:.14em;color:var(--accent);margin-bottom:3em}}
.toc{{break-after:page;min-height:80vh}}.toc ol{{padding:0;list-style:none}}.toc li{{border-bottom:1px solid var(--rule);padding:.7em 0}}.toc a{{display:block;text-decoration:none}}
@page{{margin:22mm 18mm}} @media print{{main{{padding:0}}}}
</style></head><body><main>{toc_page}{body}</main></body></html>"#,
        paper = theme.paper,
        ink = theme.ink,
        muted = theme.muted,
        accent = theme.accent,
        rule = theme.rule,
        code = theme.code_bg,
        size = request.font_size,
        leading = theme.line_height,
        measure = theme.measure,
        gap = theme.paragraph_gap,
    )
}

fn markdown_text(markdown: &str) -> Vec<String> {
    let parser = Parser::new_ext(markdown, Options::all());
    let mut lines = Vec::new();
    let mut current = String::new();
    for event in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => current.push_str("# "),
            Event::Start(Tag::Item) => current.push_str("• "),
            Event::Text(text) | Event::Code(text) => current.push_str(&text),
            Event::SoftBreak => current.push(' '),
            Event::HardBreak => current.push('\n'),
            Event::End(TagEnd::Paragraph)
            | Event::End(TagEnd::Heading(_))
            | Event::End(TagEnd::Item)
            | Event::End(TagEnd::CodeBlock) => {
                if !current.trim().is_empty() {
                    lines.extend(current.lines().map(str::to_owned));
                    lines.push(String::new());
                }
                current.clear();
            }
            _ => {}
        }
    }
    if !current.trim().is_empty() {
        lines.push(current);
    }
    lines
}

fn headings(markdown: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut in_heading = false;
    for event in Parser::new_ext(markdown, Options::all()) {
        match event {
            Event::Start(Tag::Heading { level, .. }) if level <= HeadingLevel::H3 => {
                in_heading = true;
                current.clear();
            }
            Event::Text(text) if in_heading => current.push_str(&text),
            Event::End(TagEnd::Heading(_)) if in_heading => {
                if !current.is_empty() {
                    output.push(current.clone());
                }
                in_heading = false;
            }
            _ => {}
        }
    }
    output
}

fn write_pdf(path: &Path, request: &ExportRequest) -> Result<(), String> {
    let size = request.font_size.clamp(9.0, 20.0);
    let chars_per_line = (82.0 * (12.0 / size)).round().max(38.0) as usize;
    let lines_per_page = (55.0 * (12.0 / size)).round().clamp(34.0, 65.0) as usize;
    let mut pages: Vec<Vec<String>> = Vec::new();
    if request.export_options.toc {
        let mut toc = vec!["CONTENTS".to_string(), String::new()];
        for (index, file) in request.files.iter().enumerate() {
            toc.push(format!("{:02}   {}", index + 1, file.name));
            for heading in headings(&file.content).into_iter().take(5) {
                toc.push(format!("      {heading}"));
            }
        }
        pages.push(toc);
    }
    for file in &request.files {
        let mut source_lines = vec![file.name.to_uppercase(), String::new()];
        for line in markdown_text(&file.content) {
            source_lines.extend(wrap_text(&line, chars_per_line));
        }
        if source_lines.is_empty() {
            source_lines.push(String::new());
        }
        for chunk in source_lines.chunks(lines_per_page) {
            pages.push(chunk.to_vec());
        }
    }
    if pages.is_empty() {
        pages.push(vec![request.name.clone()]);
    }

    let (paper_r, paper_g, paper_b) = rgb(&request.theme.paper);
    let (ink_r, ink_g, ink_b) = rgb(&request.theme.ink);
    let (accent_r, accent_g, accent_b) = rgb(&request.theme.accent);
    let page_count = pages.len();
    let mut objects = Vec::<Vec<u8>>::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", 4 + index * 2))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());

    for (index, lines) in pages.iter().enumerate() {
        let page_id = 4 + index * 2;
        let content_id = page_id + 1;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        let mut stream = format!(
            "{paper_r:.3} {paper_g:.3} {paper_b:.3} rg 0 0 595 842 re f\n{ink_r:.3} {ink_g:.3} {ink_b:.3} rg\nBT /F1 {size:.1} Tf {leading:.1} TL 62 774 Td\n",
            leading = size * request.theme.line_height
        );
        for (line_index, line) in lines.iter().enumerate() {
            if line_index == 0 {
                stream.push_str(&format!(
                    "{accent_r:.3} {accent_g:.3} {accent_b:.3} rg /F1 {:.1} Tf ",
                    size * 1.28
                ));
            } else if line_index == 1 {
                stream.push_str(&format!(
                    "{ink_r:.3} {ink_g:.3} {ink_b:.3} rg /F1 {size:.1} Tf "
                ));
            }
            stream.push_str(&format!("({}) Tj T*\n", pdf_escape(line)));
        }
        stream.push_str("ET\n");
        let header = token_text(&request.export_options.header_center, request, index + 1);
        let footer_template = if request.export_options.page_numbers
            && request.export_options.footer_center.is_empty()
        {
            "{page}"
        } else {
            &request.export_options.footer_center
        };
        let footer = token_text(footer_template, request, index + 1);
        if !header.is_empty() {
            stream.push_str(&format!(
                "{ink_r:.3} {ink_g:.3} {ink_b:.3} rg BT /F1 8 Tf 250 814 Td ({}) Tj ET\n",
                pdf_escape(&header)
            ));
        }
        if !footer.is_empty() {
            stream.push_str(&format!(
                "{ink_r:.3} {ink_g:.3} {ink_b:.3} rg BT /F1 8 Tf 290 28 Td ({}) Tj ET\n",
                pdf_escape(&footer)
            ));
        }
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{}endstream",
                stream.len(),
                stream
            )
            .into_bytes(),
        );
    }

    let mut output = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        output.extend_from_slice(object);
        output.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = output.len();
    output.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    output.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        output.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    output.extend_from_slice(
        format!(
            "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    fs::write(path, output).map_err(|error| error.to_string())
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + word.chars().count() + 1 > width {
            lines.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn rgb(hex: &str) -> (f32, f32, f32) {
    let value = hex.trim_start_matches('#');
    if value.len() != 6 {
        return (0.0, 0.0, 0.0);
    }
    let channel = |range| u8::from_str_radix(&value[range], 16).unwrap_or(0) as f32 / 255.0;
    (channel(0..2), channel(2..4), channel(4..6))
}

fn pdf_escape(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '(' => "\\(".to_string(),
            ')' => "\\)".to_string(),
            '\\' => "\\\\".to_string(),
            '\t' => "    ".to_string(),
            value if value.is_ascii() => value.to_string(),
            _ => "?".to_string(),
        })
        .collect()
}

fn token_text(template: &str, request: &ExportRequest, page: usize) -> String {
    template
        .replace("{title}", &request.name)
        .replace("{page}", &page.to_string())
        .replace(
            "{date}",
            &chrono::Local::now().format("%Y-%m-%d").to_string(),
        )
}

fn write_docx(path: &Path, request: &ExportRequest) -> Result<(), String> {
    let mut document_body = String::new();
    if request.export_options.toc {
        document_body.push_str(
            "<w:p><w:pPr><w:pStyle w:val=\"Title\"/></w:pPr><w:r><w:t>Contents</w:t></w:r></w:p>",
        );
        for file in &request.files {
            document_body.push_str(&word_paragraph(&file.name, Some("Heading2")));
            for heading in headings(&file.content).into_iter().take(8) {
                document_body.push_str(&word_paragraph(&format!("    {heading}"), None));
            }
        }
        document_body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
    }
    for (file_index, file) in request.files.iter().enumerate() {
        if file_index > 0 {
            document_body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
        }
        document_body.push_str(&word_paragraph(&file.name, Some("Title")));
        for line in markdown_text(&file.content) {
            let (style, text) = if let Some(heading) = line.strip_prefix("# ") {
                (Some("Heading1"), heading)
            } else {
                (None, line.as_str())
            };
            document_body.push_str(&word_paragraph(text, style));
        }
    }
    let font_half_points = (request.font_size * 2.0).round() as u32;
    let color = request.theme.ink.trim_start_matches('#');
    let accent = request.theme.accent.trim_start_matches('#');
    let header = word_header_footer(
        &[
            &request.export_options.header_left,
            &request.export_options.header_center,
            &request.export_options.header_right,
        ],
        request,
        false,
    );
    let footer = word_header_footer(
        &[
            &request.export_options.footer_left,
            &request.export_options.footer_center,
            &request.export_options.footer_right,
        ],
        request,
        request.export_options.page_numbers,
    );
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{document_body}<w:sectPr><w:headerReference w:type="default" r:id="rId1"/><w:footerReference w:type="default" r:id="rId2"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1224" w:right="1296" w:bottom="1224" w:left="1296"/></w:sectPr></w:body></w:document>"#
    );
    let styles = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="{font_half_points}"/><w:color w:val="{color}"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="180" w:line="360" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:rPr><w:color w:val="{accent}"/><w:sz w:val="52"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:rPr><w:b/><w:sz w:val="34"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style></w:styles>"#
    );
    let content_types = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/></Types>"#;
    let root_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let document_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let entries = vec![
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/header1.xml", header.as_bytes()),
        ("word/footer1.xml", footer.as_bytes()),
        ("word/_rels/document.xml.rels", document_rels.as_bytes()),
    ];
    write_store_zip(path, &entries)
}

fn word_paragraph(text: &str, style: Option<&str>) -> String {
    let style_xml = style
        .map(|value| format!("<w:pPr><w:pStyle w:val=\"{value}\"/></w:pPr>"))
        .unwrap_or_default();
    format!(
        "<w:p>{style_xml}<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        escape_xml(text)
    )
}

fn word_header_footer(fields: &[&String; 3], request: &ExportRequest, force_page: bool) -> String {
    let tag = if force_page || fields.iter().any(|value| value.contains("{page}")) {
        "ftr"
    } else {
        "hdr"
    };
    let cells = fields
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let alignment = ["left", "center", "right"][index];
            let text = token_text(value, request, 1);
            let content = if value.contains("{page}") || (force_page && index == 1 && value.is_empty()) {
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#.to_string()
            } else {
                format!("<w:r><w:t>{}</w:t></w:r>", escape_xml(&text))
            };
            format!(r#"<w:tc><w:p><w:pPr><w:jc w:val="{alignment}"/></w:pPr>{content}</w:p></w:tc>"#)
        })
        .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{tag} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="3240"/><w:gridCol w:w="3240"/><w:gridCol w:w="3240"/></w:tblGrid><w:tr>{cells}</w:tr></w:tbl></w:{tag}>"#
    )
}

fn write_store_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<(), String> {
    let mut output = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let offset = output.len() as u32;
        output.extend_from_slice(&0x04034b50u32.to_le_bytes());
        output.extend_from_slice(&20u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&(data.len() as u32).to_le_bytes());
        output.extend_from_slice(&(data.len() as u32).to_le_bytes());
        output.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes());
        output.extend_from_slice(name_bytes);
        output.extend_from_slice(data);

        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let central_offset = output.len() as u32;
    let central_size = central.len() as u32;
    output.extend_from_slice(&central);
    output.extend_from_slice(&0x06054b50u32.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    output.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    output.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    output.extend_from_slice(&central_size.to_le_bytes());
    output.extend_from_slice(&central_offset.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    let mut file = fs::File::create(path).map_err(|error| error.to_string())?;
    file.write_all(&output).map_err(|error| error.to_string())
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml(value: &str) -> String {
    escape_html(value).replace('\'', "&apos;")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            render_markdown,
            read_markdown_paths,
            save_project,
            list_projects,
            load_project,
            export_document
        ])
        .run(tauri::generate_context!())
        .expect("error while running Markdown Studio");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_without_losing_words() {
        assert_eq!(wrap_text("one two three", 7), vec!["one two", "three"]);
    }

    #[test]
    fn creates_zip_signature() {
        let path = std::env::temp_dir().join("markdown-studio-test.docx");
        write_store_zip(&path, &[("hello.txt", b"hello")]).unwrap();
        assert_eq!(&fs::read(&path).unwrap()[..4], b"PK\x03\x04");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn combines_markdown_sources_in_order() {
        let files = vec![
            StudioFile {
                name: "One".into(),
                path: None,
                content: "# One\n\nFirst.\n".into(),
            },
            StudioFile {
                name: "Two".into(),
                path: None,
                content: "# Two\n\nSecond.".into(),
            },
        ];
        assert_eq!(
            build_markdown(&files),
            "# One\n\nFirst.\n\n# Two\n\nSecond.\n"
        );
    }

    #[test]
    fn imports_hundreds_of_markdown_files() {
        let directory =
            std::env::temp_dir().join(format!("markdown-studio-many-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        for index in 0..250 {
            fs::write(
                directory.join(format!("{index:03}-section.md")),
                format!("# Section {index}\n\nContent for section {index}."),
            )
            .unwrap();
        }
        let imported =
            read_markdown_paths_blocking(vec![directory.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(imported.len(), 250);
        assert_eq!(imported.first().unwrap().name, "000-section");
        assert_eq!(imported.last().unwrap().name, "249-section");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn renders_a_large_markdown_document() {
        let mut markdown = String::from("# Large document\n\n");
        for index in 0..20_000 {
            markdown.push_str(&format!(
                "Paragraph {index} with **formatted** content.\n\n"
            ));
        }
        let html = markdown_html(&markdown);
        assert!(html.starts_with("<h1>Large document</h1>"));
        assert!(html.contains("Paragraph 19999"));
    }
}

use pulldown_cmark::{html, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, MutexGuard,
    },
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const MAIN_WINDOW: &str = "main";
const OPEN_FILES_EVENT: &str = "open-files-requested";
static NEXT_WINDOW_NUMBER: AtomicUsize = AtomicUsize::new(1);

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
    #[serde(default = "default_page_number_position")]
    page_number_position: String,
    #[serde(default = "default_page_number_format")]
    page_number_format: String,
    #[serde(default = "default_margins")]
    margins: String,
    /// Printed body text size in points, independent of the on-screen reader size.
    #[serde(default = "default_text_size")]
    text_size: f32,
    #[serde(default)]
    section_breaks: bool,
    #[serde(default)]
    header_left: String,
    #[serde(default)]
    header_center: String,
    #[serde(default)]
    header_right: String,
    #[serde(default)]
    footer_left: String,
    #[serde(default)]
    footer_center: String,
    #[serde(default)]
    footer_right: String,
}

fn default_page_number_position() -> String {
    "center".to_string()
}

fn default_page_number_format() -> String {
    "number".to_string()
}

fn default_margins() -> String {
    "normal".to_string()
}

fn default_text_size() -> f32 {
    11.0
}

fn printed_text_size(options: &ExportOptions) -> f32 {
    options.text_size.clamp(8.0, 16.0)
}

/// Ink colours for paper output. Pages carry no background, so a dark theme's light text would
/// vanish: it uses dark ink and a darkened accent instead.
struct PrintPalette {
    ink: String,
    muted: String,
    accent: String,
}

fn print_palette(theme: &ThemeTokens) -> PrintPalette {
    if theme.mode != "dark" {
        return PrintPalette {
            ink: theme.ink.clone(),
            muted: theme.muted.clone(),
            accent: theme.accent.clone(),
        };
    }
    let (red, green, blue) = rgb(&theme.accent);
    let darken = |channel: f32| (channel * 0.58 * 255.0).round() as u8;
    PrintPalette {
        ink: "#161616".to_string(),
        muted: "#666666".to_string(),
        accent: format!("#{:02x}{:02x}{:02x}", darken(red), darken(green), darken(blue)),
    }
}

/// Page margins in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PageMargins {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

fn page_margins(preset: &str) -> PageMargins {
    match preset {
        "narrow" => PageMargins { top: 12.7, right: 12.7, bottom: 12.7, left: 12.7 },
        "wide" => PageMargins { top: 25.4, right: 50.8, bottom: 25.4, left: 50.8 },
        _ => PageMargins { top: 25.4, right: 25.4, bottom: 25.4, left: 25.4 },
    }
}

fn page_number_template(options: &ExportOptions) -> Option<&'static str> {
    if !options.page_numbers {
        return None;
    }
    Some(match options.page_number_format.as_str() {
        "page" => "Page {page}",
        "page-of" => "Page {page} of {pages}",
        _ => "{page}",
    })
}

/// Header and footer templates for the left, center, and right slots. When page numbers are
/// enabled they are merged into the chosen footer slot, after any text already there.
fn running_slots(options: &ExportOptions) -> ([String; 3], [String; 3]) {
    let header = [&options.header_left, &options.header_center, &options.header_right]
        .map(|value| value.trim().to_string());
    let mut footer = [&options.footer_left, &options.footer_center, &options.footer_right]
        .map(|value| value.trim().to_string());
    if let Some(template) = page_number_template(options) {
        let slot = match options.page_number_position.as_str() {
            "left" => 0,
            "right" => 2,
            _ => 1,
        };
        footer[slot] = if footer[slot].is_empty() {
            template.to_string()
        } else {
            format!("{} · {template}", footer[slot])
        };
    }
    (header, footer)
}

fn default_content_width() -> String {
    "theme".to_string()
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
    #[serde(default = "default_content_width")]
    content_width: String,
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

/// Per-window bookkeeping: Markdown paths waiting to be loaded by a window, and the windows that
/// currently show the empty welcome screen and can therefore take the next opened file.
#[derive(Default)]
struct WindowState {
    pending: HashMap<String, Vec<String>>,
    idle: HashSet<String>,
}

#[derive(Default)]
struct Windows(Mutex<WindowState>);

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

fn open_paths_from_arguments(
    arguments: impl IntoIterator<Item = String>,
    working_directory: Option<&Path>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for argument in arguments {
        if argument.starts_with('-') {
            continue;
        }
        let mut path = if argument.starts_with("file://") {
            match tauri::Url::parse(&argument)
                .ok()
                .and_then(|url| url.to_file_path().ok())
            {
                Some(path) => path,
                None => continue,
            }
        } else {
            PathBuf::from(argument)
        };
        if path.is_relative() {
            if let Some(directory) = working_directory {
                path = directory.join(path);
            }
        }
        if is_markdown_path(&path) && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn lock_windows(app: &AppHandle) -> MutexGuard<'_, WindowState> {
    app.state::<Windows>()
        .inner()
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn queue_for_window(state: &mut WindowState, label: &str, paths: Vec<PathBuf>) {
    let queue = state.pending.entry(label.to_string()).or_default();
    for path in paths {
        let path = path.to_string_lossy().into_owned();
        if !queue.contains(&path) {
            queue.push(path);
        }
    }
    state.idle.remove(label);
}

fn focus_window(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Sends files opened from the operating system (or a repeated launch) to a window. An idle
/// window on the welcome screen is reused; otherwise a new window is opened so documents that
/// are already open stay untouched.
fn route_open_request(app: &AppHandle, paths: Vec<PathBuf>) {
    let reused = {
        let mut state = lock_windows(app);
        let mut idle_labels: Vec<String> = state.idle.iter().cloned().collect();
        idle_labels.sort_by_key(|label| label != MAIN_WINDOW);
        let target = idle_labels
            .into_iter()
            .find_map(|label| app.get_webview_window(&label).map(|window| (label, window)));
        if let Some((label, _)) = &target {
            if !paths.is_empty() {
                queue_for_window(&mut state, label, paths.clone());
            }
        }
        target
    };
    match reused {
        Some((label, window)) => {
            if !paths.is_empty() {
                let _ = app.emit(OPEN_FILES_EVENT, label);
            }
            focus_window(&window);
        }
        None => spawn_document_window(app.clone(), paths),
    }
}

/// Window creation deadlocks on Windows when it runs inside an event handler, so it is moved
/// onto the async runtime.
fn spawn_document_window(app: AppHandle, paths: Vec<PathBuf>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = create_document_window(&app, paths) {
            eprintln!("Unable to open a Markdown Studio window: {error}");
        }
    });
}

fn create_document_window(app: &AppHandle, paths: Vec<PathBuf>) -> Result<WebviewWindow, String> {
    let label = loop {
        let candidate = format!(
            "document-{}",
            NEXT_WINDOW_NUMBER.fetch_add(1, Ordering::Relaxed)
        );
        if app.get_webview_window(&candidate).is_none() {
            break candidate;
        }
    };
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .cloned()
        .ok_or("The main window configuration is missing")?;
    config.label = label.clone();
    if let Some((x, y)) = cascade_position(app, config.width, config.height) {
        config.center = false;
        config.x = Some(x);
        config.y = Some(y);
    }
    {
        let mut state = lock_windows(app);
        if paths.is_empty() {
            state.idle.insert(label.clone());
        } else {
            queue_for_window(&mut state, &label, paths);
        }
    }
    let window = WebviewWindowBuilder::from_config(app, &config)
        .and_then(|builder| builder.build())
        .map_err(|error| {
            let mut state = lock_windows(app);
            state.pending.remove(&label);
            state.idle.remove(&label);
            error.to_string()
        })?;
    focus_window(&window);
    Ok(window)
}

/// Offsets a new window from the focused one so it does not land exactly on top of it, wrapping
/// back to the monitor's corner when the cascade would run off screen.
fn cascade_position(app: &AppHandle, width: f64, height: f64) -> Option<(f64, f64)> {
    let windows = app.webview_windows();
    let source = windows
        .values()
        .find(|window| window.is_focused().unwrap_or(false))
        .or_else(|| windows.get(MAIN_WINDOW))?;
    if source.is_maximized().unwrap_or(false) || source.is_minimized().unwrap_or(false) {
        return None;
    }
    let scale = source.scale_factor().ok()?;
    let position = source.outer_position().ok()?.to_logical::<f64>(scale);
    let (mut x, mut y) = (position.x + 32.0, position.y + 32.0);
    if let Some(monitor) = source.current_monitor().ok().flatten() {
        let origin = monitor.position().to_logical::<f64>(monitor.scale_factor());
        let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
        if x + width > origin.x + size.width || y + height > origin.y + size.height {
            x = origin.x + 48.0;
            y = origin.y + 48.0;
        }
    }
    Some((x, y))
}

#[tauri::command]
fn take_opened_files(window: WebviewWindow, state: State<'_, Windows>) -> Vec<String> {
    let mut state = state.0.lock().unwrap_or_else(|error| error.into_inner());
    state.pending.remove(window.label()).unwrap_or_default()
}

/// The frontend reports whether its window is on the empty welcome screen.
#[tauri::command]
fn set_window_idle(window: WebviewWindow, idle: bool, state: State<'_, Windows>) {
    let label = window.label().to_string();
    let mut state = state.0.lock().unwrap_or_else(|error| error.into_inner());
    let has_pending = state.pending.get(&label).is_some_and(|queue| !queue.is_empty());
    if idle && !has_pending {
        state.idle.insert(label);
    } else {
        state.idle.remove(&label);
    }
}

#[tauri::command]
async fn open_new_window(app: AppHandle) -> Result<(), String> {
    create_document_window(&app, Vec::new()).map(|_| ())
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

#[tauri::command]
async fn write_markdown_file(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || write_markdown_file_blocking(path, content))
        .await
        .map_err(|error| error.to_string())?
}

fn write_markdown_file_blocking(path: String, content: String) -> Result<(), String> {
    let path = Path::new(&path);
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        if !parent.exists() {
            return Err(format!("Folder no longer exists: {}", parent.display()));
        }
    }
    fs::write(path, content).map_err(|error| format!("Unable to save {}: {error}", path.display()))
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

/// Saves the window's print layout as a PDF using the WebView's own engine, so the result matches
/// Print exactly. Returns `false` where that is unavailable and the built-in writer should be used.
#[tauri::command]
async fn print_window_to_pdf(window: WebviewWindow, path: String) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let bytes = webview_print_to_pdf(&window).await?;
        fs::write(&path, bytes).map_err(|error| format!("Unable to save {path}: {error}"))?;
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        let _ = (window, path);
        Ok(false)
    }
}

#[cfg(windows)]
async fn webview_print_to_pdf(window: &WebviewWindow) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows_core::HSTRING;

    // A4 paper. Margins and page numbers come from the page's own `@page` rule.
    const PARAMETERS: &str = r#"{"printBackground":true,"displayHeaderFooter":false,"paperWidth":8.27,"paperHeight":11.69,"generateTaggedPDF":true}"#;

    let (sender, receiver) = std::sync::mpsc::channel::<Result<String, String>>();
    let start_failure = sender.clone();
    window
        .with_webview(move |webview| {
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                move |status, json| {
                    let _ = sender.send(status.map(|_| json).map_err(|error| error.to_string()));
                    Ok(())
                },
            ));
            let started = unsafe {
                webview.controller().CoreWebView2().and_then(|core| {
                    core.CallDevToolsProtocolMethod(
                        &HSTRING::from("Page.printToPDF"),
                        &HSTRING::from(PARAMETERS),
                        &handler,
                    )
                })
            };
            if let Err(error) = started {
                let _ = start_failure.send(Err(error.to_string()));
            }
        })
        .map_err(|error| error.to_string())?;
    let json = tauri::async_runtime::spawn_blocking(move || {
        receiver.recv_timeout(std::time::Duration::from_secs(300))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|_| "Timed out while creating the PDF".to_string())??;
    let response: serde_json::Value =
        serde_json::from_str(&json).map_err(|error| error.to_string())?;
    let data = response
        .get("data")
        .and_then(|value| value.as_str())
        .ok_or("The PDF engine returned no data")?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| error.to_string())
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
    let palette = print_palette(theme);
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
{page_css} @media print{{:root{{--paper:transparent;--ink:{print_ink};--muted:{print_muted};--accent:{print_accent};--rule:#dddddd;--code:#f5f5f5}} html{{background:transparent}} body{{font-size:{print_size}pt}} main{{padding:0;max-width:none}} pre{{white-space:pre-wrap;overflow-wrap:anywhere}} h1,h2,h3,h4{{break-after:avoid;break-inside:avoid}} img,tr{{break-inside:avoid}} p,li{{orphans:3;widows:3}} .source-file,.toc{{min-height:0}}}}
</style></head><body><main>{toc_page}{body}</main></body></html>"#,
        page_css = print_page_css(request),
        print_size = printed_text_size(&request.export_options),
        print_ink = palette.ink,
        print_muted = palette.muted,
        print_accent = palette.accent,
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

/// The `@page` rule for printed HTML: margins from the chosen preset, plus header, footer, and
/// page number text in the page-margin boxes (supported by Chromium 131 and later).
fn print_page_css(request: &ExportRequest) -> String {
    let margins = page_margins(&request.export_options.margins);
    let (header, footer) = running_slots(&request.export_options);
    let mut boxes = String::new();
    for (names, slots) in [
        (["top-left", "top-center", "top-right"], header),
        (["bottom-left", "bottom-center", "bottom-right"], footer),
    ] {
        for (name, template) in names.iter().zip(slots.iter()) {
            if !template.is_empty() {
                boxes.push_str(&format!(
                    "@{name}{{content:{};font:8.5pt ui-sans-serif,system-ui,sans-serif;color:{}}}",
                    css_content(template, request),
                    print_palette(&request.theme).muted
                ));
            }
        }
    }
    format!(
        "@page{{margin:{}mm {}mm {}mm {}mm;{boxes}}}",
        margins.top, margins.right, margins.bottom, margins.left
    )
}

/// Converts a header or footer template into a CSS `content` value, mapping `{page}` and
/// `{pages}` to the page counters.
fn css_content(template: &str, request: &ExportRequest) -> String {
    let text = template
        .replace("{title}", &request.name)
        .replace("{date}", &chrono::Local::now().format("%Y-%m-%d").to_string());
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let counter = if rest.starts_with("{pages}") {
            Some(("counter(pages)", 7))
        } else if rest.starts_with("{page}") {
            Some(("counter(page)", 6))
        } else {
            None
        };
        if let Some((counter, length)) = counter {
            if !literal.is_empty() {
                parts.push(css_string(&std::mem::take(&mut literal)));
            }
            parts.push(counter.to_string());
            rest = &rest[length..];
        } else {
            let character = rest.chars().next().unwrap();
            literal.push(character);
            rest = &rest[character.len_utf8()..];
        }
    }
    if !literal.is_empty() || parts.is_empty() {
        parts.push(css_string(&literal));
    }
    parts.join(" ")
}

fn css_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '<' => output.push_str("\\3c "),
            '\n' | '\r' => output.push(' '),
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn markdown_text(markdown: &str) -> Vec<String> {
    let parser = Parser::new_ext(markdown, Options::all());
    let mut lines = Vec::new();
    let mut current = String::new();
    for event in parser {
        match event {
            Event::Start(Tag::Heading { .. }) => current.push_str("# "),
            Event::Start(Tag::Item) => {
                // A nested list starts while its parent item's text is still pending.
                if !current.trim().is_empty() {
                    lines.extend(current.lines().map(str::to_owned));
                }
                current.clear();
                current.push_str("• ");
            }
            Event::Text(text) | Event::Code(text) => current.push_str(&text),
            Event::SoftBreak => current.push(' '),
            Event::HardBreak => current.push('\n'),
            Event::End(TagEnd::TableCell) => current.push_str(" | "),
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                lines.push(current.trim_end_matches(" | ").to_owned());
                current.clear();
            }
            Event::End(TagEnd::Table) => lines.push(String::new()),
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

const PDF_PAGE_WIDTH: f32 = 595.0;
const PDF_PAGE_HEIGHT: f32 = 842.0;
const PT_PER_MM: f32 = 72.0 / 25.4;
const PDF_RUNNING_SIZE: f32 = 8.5;

/// Advance widths of Helvetica for ASCII 32..=126, in 1/1000 em (Adobe core font metrics).
const HELVETICA_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum PdfStyle {
    Title,
    Heading,
    Body,
}

#[derive(Debug, Clone, PartialEq)]
struct PdfLine {
    style: PdfStyle,
    text: String,
    x: f32,
    baseline: f32,
    size: f32,
}

/// Text width in points as drawn after `pdf_escape` (tabs become four spaces, other non-ASCII
/// characters become `?`). Bold text is estimated slightly wider than regular Helvetica.
fn text_width(text: &str, size: f32, bold: bool) -> f32 {
    let units: u32 = text
        .chars()
        .map(|character| match character {
            ' '..='~' => HELVETICA_WIDTHS[character as usize - 32] as u32,
            '\t' => 4 * 278,
            '•' => 350,
            '—' | '…' | '™' | '‰' => 1000,
            '‘' | '’' | '‚' => 222,
            '“' | '”' | '„' => 333,
            _ => 556,
        })
        .sum();
    units as f32 / 1000.0 * size * if bold { 1.1 } else { 1.0 }
}

/// Places lines top to bottom inside the page margins, starting a new page whenever the next
/// line would cross the bottom margin.
struct PdfPaginator {
    pages: Vec<Vec<PdfLine>>,
    current: Vec<PdfLine>,
    cursor: f32,
    top: f32,
    bottom: f32,
    left: f32,
    width: f32,
    base_size: f32,
    line_height: f32,
}

impl PdfPaginator {
    fn new(margins: PageMargins, base_size: f32, line_height: f32) -> Self {
        let top = PDF_PAGE_HEIGHT - margins.top * PT_PER_MM;
        Self {
            pages: Vec::new(),
            current: Vec::new(),
            cursor: top,
            top,
            bottom: margins.bottom * PT_PER_MM,
            left: margins.left * PT_PER_MM,
            width: PDF_PAGE_WIDTH - (margins.left + margins.right) * PT_PER_MM,
            base_size,
            line_height,
        }
    }

    fn font_size(&self, style: PdfStyle) -> f32 {
        match style {
            PdfStyle::Title => self.base_size * 1.28,
            PdfStyle::Heading => self.base_size * 1.12,
            PdfStyle::Body => self.base_size,
        }
    }

    fn page_break(&mut self) {
        if !self.current.is_empty() {
            self.pages.push(std::mem::take(&mut self.current));
        }
        self.cursor = self.top;
    }

    fn gap(&mut self, amount: f32) {
        if !self.current.is_empty() {
            self.cursor -= amount;
        }
    }

    fn paragraph(&mut self, style: PdfStyle, text: &str, indent: f32) {
        let size = self.font_size(style);
        let bold = style != PdfStyle::Body;
        let available = (self.width - indent).max(size * 4.0);
        let advance = size * self.line_height;
        for line in wrap_text(text, |candidate| text_width(candidate, size, bold) <= available) {
            if self.cursor - advance < self.bottom && !self.current.is_empty() {
                self.page_break();
            }
            // Centre the glyphs in the line box so ascenders and descenders stay inside it.
            let baseline = self.cursor - (advance - size) / 2.0 - size * 0.8;
            self.cursor -= advance;
            self.current.push(PdfLine {
                style,
                text: line,
                x: self.left + indent,
                baseline,
                size,
            });
        }
    }

    fn finish(mut self) -> Vec<Vec<PdfLine>> {
        self.page_break();
        self.pages
    }
}

fn layout_pdf(request: &ExportRequest) -> Vec<Vec<PdfLine>> {
    let margins = page_margins(&request.export_options.margins);
    let size = printed_text_size(&request.export_options);
    let line_height = request.theme.line_height.clamp(1.0, 2.4);
    let paragraph_gap = size * request.theme.paragraph_gap.clamp(0.3, 2.0) * 0.8;
    let mut layout = PdfPaginator::new(margins, size, line_height);
    if request.export_options.toc {
        layout.paragraph(PdfStyle::Title, "CONTENTS", 0.0);
        layout.gap(paragraph_gap);
        for (index, file) in request.files.iter().enumerate() {
            layout.paragraph(PdfStyle::Body, &format!("{:02}   {}", index + 1, file.name), 0.0);
            for heading in headings(&file.content).into_iter().take(5) {
                layout.paragraph(PdfStyle::Body, &heading, size * 2.4);
            }
        }
        layout.page_break();
    }
    for file in &request.files {
        layout.paragraph(PdfStyle::Title, &file.name.to_uppercase(), 0.0);
        layout.gap(paragraph_gap);
        for line in markdown_text(&file.content) {
            if line.is_empty() {
                layout.gap(paragraph_gap);
            } else if let Some(heading) = line.strip_prefix("# ") {
                layout.gap(paragraph_gap * 0.5);
                layout.paragraph(PdfStyle::Heading, heading, 0.0);
            } else {
                layout.paragraph(PdfStyle::Body, &line, 0.0);
            }
        }
        layout.page_break();
    }
    let pages = layout.finish();
    if !pages.is_empty() {
        return pages;
    }
    let mut fallback = PdfPaginator::new(margins, size, line_height);
    fallback.paragraph(PdfStyle::Title, &request.name, 0.0);
    fallback.finish()
}

fn write_pdf(path: &Path, request: &ExportRequest) -> Result<(), String> {
    let pages = layout_pdf(request);
    let margins = page_margins(&request.export_options.margins);
    let (header, footer) = running_slots(&request.export_options);
    let palette = print_palette(&request.theme);
    let (ink_r, ink_g, ink_b) = rgb(&palette.ink);
    let (accent_r, accent_g, accent_b) = rgb(&palette.accent);
    let (muted_r, muted_g, muted_b) = rgb(&palette.muted);
    let page_count = pages.len();
    let mut objects = Vec::<Vec<u8>>::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", 5 + index * 2))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes());
    objects.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    );
    objects.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );

    // Running text sits in the middle of the top and bottom margins.
    let header_baseline =
        PDF_PAGE_HEIGHT - margins.top * PT_PER_MM / 2.0 - PDF_RUNNING_SIZE * 0.35;
    let footer_baseline = margins.bottom * PT_PER_MM / 2.0 - PDF_RUNNING_SIZE * 0.35;
    let left_edge = margins.left * PT_PER_MM;
    let right_edge = PDF_PAGE_WIDTH - margins.right * PT_PER_MM;

    for (index, lines) in pages.iter().enumerate() {
        let content_id = 6 + index * 2;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PDF_PAGE_WIDTH} {PDF_PAGE_HEIGHT}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {content_id} 0 R >>"
            )
            .into_bytes(),
        );
        let mut stream = String::new();
        for line in lines {
            let (font, (red, green, blue)) = match line.style {
                PdfStyle::Title => ("F2", (accent_r, accent_g, accent_b)),
                PdfStyle::Heading => ("F2", (ink_r, ink_g, ink_b)),
                PdfStyle::Body => ("F1", (ink_r, ink_g, ink_b)),
            };
            stream.push_str(&format!(
                "{red:.3} {green:.3} {blue:.3} rg BT /{font} {:.2} Tf {:.2} {:.2} Td ({}) Tj ET\n",
                line.size,
                line.x,
                line.baseline,
                pdf_escape(&line.text)
            ));
        }
        for (slots, baseline) in [(&header, header_baseline), (&footer, footer_baseline)] {
            for (slot, template) in slots.iter().enumerate() {
                let text = token_text(template, request, index + 1, page_count);
                if text.is_empty() {
                    continue;
                }
                let width = text_width(&text, PDF_RUNNING_SIZE, false);
                let x = match slot {
                    0 => left_edge,
                    1 => (PDF_PAGE_WIDTH - width) / 2.0,
                    _ => right_edge - width,
                };
                stream.push_str(&format!(
                    "{muted_r:.3} {muted_g:.3} {muted_b:.3} rg BT /F1 {PDF_RUNNING_SIZE} Tf {x:.2} {baseline:.2} Td ({}) Tj ET\n",
                    pdf_escape(&text)
                ));
            }
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

/// Greedy word wrap. `fits` decides whether a candidate line is narrow enough; a word that is
/// too long on its own is split across lines so it never runs into the margin.
fn wrap_text(text: &str, fits: impl Fn(&str) -> bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if fits(&candidate) {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for character in word.chars() {
            current.push(character);
            if current.chars().count() > 1 && !fits(&current) {
                current.pop();
                lines.push(std::mem::replace(&mut current, character.to_string()));
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
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
            value => match win_ansi_byte(value) {
                Some(byte) => format!("\\{byte:03o}"),
                None => "?".to_string(),
            },
        })
        .collect()
}

/// The WinAnsiEncoding byte for a non-ASCII character, covering Latin letters and typographic
/// punctuation such as dashes, curly quotes, and bullets.
fn win_ansi_byte(character: char) -> Option<u8> {
    Some(match character {
        '\u{A0}'..='\u{FF}' => character as u32 as u8,
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => return None,
    })
}

fn token_text(template: &str, request: &ExportRequest, page: usize, pages: usize) -> String {
    template
        .replace("{title}", &request.name)
        .replace("{pages}", &pages.to_string())
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
    let font_half_points = (printed_text_size(&request.export_options) * 2.0).round() as u32;
    let palette = print_palette(&request.theme);
    let color = palette.ink.trim_start_matches('#');
    let accent = palette.accent.trim_start_matches('#');
    let margins = page_margins(&request.export_options.margins);
    let twips = |millimetres: f32| (millimetres * 1440.0 / 25.4).round() as u32;
    let (top, right, bottom, left) = (
        twips(margins.top),
        twips(margins.right),
        twips(margins.bottom),
        twips(margins.left),
    );
    let content_width = 12240u32.saturating_sub(left + right).max(1440);
    let (header_slots, footer_slots) = running_slots(&request.export_options);
    let header = word_header_footer("hdr", &header_slots, request, content_width);
    let footer = word_header_footer("ftr", &footer_slots, request, content_width);
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{document_body}<w:sectPr><w:headerReference w:type="default" r:id="rId1"/><w:footerReference w:type="default" r:id="rId2"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="{top}" w:right="{right}" w:bottom="{bottom}" w:left="{left}" w:header="{}" w:footer="{}" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        top / 2,
        bottom / 2
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

/// Builds a header (`hdr`) or footer (`ftr`) part with left, center, and right cells. `{page}`
/// and `{pages}` become live PAGE and NUMPAGES fields so Word numbers every page.
fn word_header_footer(
    tag: &str,
    slots: &[String; 3],
    request: &ExportRequest,
    content_width: u32,
) -> String {
    let palette = print_palette(&request.theme);
    let muted = palette.muted.trim_start_matches('#');
    let run_properties = format!(r#"<w:rPr><w:color w:val="{muted}"/><w:sz w:val="17"/></w:rPr>"#);
    let column = content_width / 3;
    let cells = slots
        .iter()
        .enumerate()
        .map(|(index, template)| {
            let alignment = ["left", "center", "right"][index];
            let runs = word_template_runs(template, request, &run_properties);
            format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="{column}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:jc w:val="{alignment}"/></w:pPr>{runs}</w:p></w:tc>"#
            )
        })
        .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{tag} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:tbl><w:tblPr><w:tblW w:w="{width}" w:type="dxa"/><w:tblLayout w:type="fixed"/></w:tblPr><w:tblGrid><w:gridCol w:w="{column}"/><w:gridCol w:w="{column}"/><w:gridCol w:w="{column}"/></w:tblGrid><w:tr>{cells}</w:tr></w:tbl><w:p><w:pPr><w:spacing w:after="0" w:line="20" w:lineRule="exact"/></w:pPr></w:p></w:{tag}>"#,
        width = column * 3
    )
}

fn word_template_runs(template: &str, request: &ExportRequest, run_properties: &str) -> String {
    let text = template
        .replace("{title}", &request.name)
        .replace("{date}", &chrono::Local::now().format("%Y-%m-%d").to_string());
    let literal = |value: &str| {
        if value.is_empty() {
            String::new()
        } else {
            format!(
                r#"<w:r>{run_properties}<w:t xml:space="preserve">{}</w:t></w:r>"#,
                escape_xml(value)
            )
        }
    };
    let field = |instruction: &str| {
        format!(
            r#"<w:r>{run_properties}<w:fldChar w:fldCharType="begin"/></w:r><w:r>{run_properties}<w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r>{run_properties}<w:fldChar w:fldCharType="separate"/></w:r><w:r>{run_properties}<w:t>1</w:t></w:r><w:r>{run_properties}<w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let mut output = String::new();
    let mut rest = text.as_str();
    loop {
        let next = [("{pages}", "NUMPAGES"), ("{page}", "PAGE")]
            .into_iter()
            .filter_map(|(token, instruction)| {
                rest.find(token).map(|index| (index, token, instruction))
            })
            .min_by_key(|(index, token, _)| (*index, std::cmp::Reverse(token.len())));
        let Some((index, token, instruction)) = next else {
            output.push_str(&literal(rest));
            break;
        };
        output.push_str(&literal(&rest[..index]));
        output.push_str(&field(instruction));
        rest = &rest[index + token.len()..];
    }
    output
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
    let mut builder = tauri::Builder::default().manage(Windows::default());

    // A second launch (for example "Open with" on another file) is forwarded to this process,
    // which opens it in its own window so every document gets an independent workspace.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(
            |app, arguments, working_directory| {
                let paths = open_paths_from_arguments(
                    arguments.into_iter().skip(1),
                    Some(Path::new(&working_directory)),
                );
                route_open_request(app, paths);
            },
        ));
    }

    let application = builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let paths = startup_paths();
            let mut state = lock_windows(app.handle());
            if paths.is_empty() {
                state.idle.insert(MAIN_WINDOW.to_string());
            } else {
                queue_for_window(&mut state, MAIN_WINDOW, paths);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                let mut state = lock_windows(window.app_handle());
                state.pending.remove(window.label());
                state.idle.remove(window.label());
            }
        })
        .invoke_handler(tauri::generate_handler![
            take_opened_files,
            set_window_idle,
            open_new_window,
            print_window_to_pdf,
            render_markdown,
            read_markdown_paths,
            write_markdown_file,
            save_project,
            list_projects,
            load_project,
            export_document
        ])
        .build(tauri::generate_context!())
        .expect("error while building Markdown Studio");

    application.run(
        #[allow(unused_variables)]
        |app, event| {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            if let tauri::RunEvent::Opened { urls } = event {
                let paths = urls
                    .into_iter()
                    .filter_map(|url| url.to_file_path().ok())
                    .filter(|path| is_markdown_path(path))
                    .collect::<Vec<_>>();
                if !paths.is_empty() {
                    route_open_request(app, paths);
                }
            }
        },
    );
}

/// Markdown files passed on the command line when the app starts (Windows and Linux; macOS
/// delivers them through `RunEvent::Opened`).
fn startup_paths() -> Vec<PathBuf> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        let working_directory = std::env::current_dir().ok();
        open_paths_from_arguments(std::env::args().skip(1), working_directory.as_deref())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fits_chars(width: usize) -> impl Fn(&str) -> bool {
        move |line: &str| line.chars().count() <= width
    }

    fn sample_request(options: ExportOptions, files: Vec<StudioFile>) -> ExportRequest {
        ExportRequest {
            id: None,
            name: "Field \"Notes\"".into(),
            files,
            theme: ThemeTokens {
                id: "classic".into(),
                name: "Classic".into(),
                mode: "light".into(),
                paper: "#fbfaf6".into(),
                ink: "#272621".into(),
                muted: "#77736a".into(),
                accent: "#b64a2e".into(),
                rule: "#d8d3c8".into(),
                code_bg: "#f0ede5".into(),
                font_body: "serif".into(),
                font_heading: "sans".into(),
                base_size: 18.0,
                line_height: 1.72,
                measure: 690.0,
                paragraph_gap: 1.15,
            },
            font_size: 18.0,
            typeface: "serif".into(),
            export_options: options,
            format: "pdf".into(),
            output_path: String::new(),
        }
    }

    fn sample_options() -> ExportOptions {
        ExportOptions {
            toc: false,
            page_numbers: true,
            page_number_position: "center".into(),
            page_number_format: "number".into(),
            margins: "normal".into(),
            text_size: 11.0,
            section_breaks: false,
            header_left: String::new(),
            header_center: "{title}".into(),
            header_right: String::new(),
            footer_left: String::new(),
            footer_center: String::new(),
            footer_right: String::new(),
        }
    }

    fn long_file() -> StudioFile {
        let mut content = String::from("# Long document\n\n");
        for index in 0..120 {
            content.push_str(&format!(
                "Paragraph {index} has enough words to wrap across several printed lines on an A4 page with generous margins.\n\n"
            ));
        }
        content.push_str("https://example.com/a-very-long-address-without-any-spaces-that-must-still-respect-the-right-margin-of-the-page\n");
        StudioFile {
            name: "Long".into(),
            path: None,
            content,
        }
    }

    #[test]
    fn wraps_without_losing_words() {
        assert_eq!(wrap_text("one two three", fits_chars(7)), vec!["one two", "three"]);
    }

    #[test]
    fn wrap_splits_words_longer_than_a_line() {
        assert_eq!(wrap_text("ab abcdefgh", fits_chars(3)), vec!["ab", "abc", "def", "gh"]);
        assert_eq!(wrap_text("", fits_chars(3)), vec![String::new()]);
    }

    #[test]
    fn page_numbers_join_their_footer_slot() {
        let mut options = sample_options();
        options.footer_center = "Draft".into();
        options.page_number_format = "page-of".into();
        let (header, footer) = running_slots(&options);
        assert_eq!(header, ["", "{title}", ""].map(String::from));
        assert_eq!(footer, ["", "Draft · Page {page} of {pages}", ""].map(String::from));

        options.page_number_position = "right".into();
        options.page_number_format = "page".into();
        assert_eq!(running_slots(&options).1, ["", "Draft", "Page {page}"].map(String::from));

        options.page_numbers = false;
        assert_eq!(running_slots(&options).1, ["", "Draft", ""].map(String::from));
    }

    #[test]
    fn legacy_export_options_default_to_centered_numbers_and_normal_margins() {
        let options: ExportOptions = serde_json::from_str(
            r#"{"toc":true,"pageNumbers":true,"headerLeft":"","headerCenter":"{title}","headerRight":"","footerLeft":"","footerCenter":"","footerRight":""}"#,
        )
        .unwrap();
        assert_eq!(options.page_number_position, "center");
        assert_eq!(options.page_number_format, "number");
        assert_eq!(page_margins(&options.margins), page_margins("normal"));
        assert_eq!(options.text_size, 11.0);
    }

    #[test]
    fn printed_text_size_ignores_the_reader_size_and_controls_the_pdf() {
        let mut options = sample_options();
        options.text_size = 9.0;
        let small = sample_request(options.clone(), vec![long_file()]);
        options.text_size = 14.0;
        let large = sample_request(options, vec![long_file()]);
        assert_eq!(small.font_size, 18.0);
        let body_size = |request: &ExportRequest| {
            layout_pdf(request)
                .iter()
                .flatten()
                .find(|line| line.style == PdfStyle::Body)
                .unwrap()
                .size
        };
        assert_eq!(body_size(&small), 9.0);
        assert_eq!(body_size(&large), 14.0);
        assert!(layout_pdf(&small).len() < layout_pdf(&large).len());
    }

    #[test]
    fn dark_themes_export_dark_ink_with_no_page_background() {
        let mut request = sample_request(sample_options(), vec![long_file()]);
        request.theme.mode = "dark".into();
        request.theme.ink = "#e9e8e1".into();
        request.theme.accent = "#efb35d".into();
        let palette = print_palette(&request.theme);
        assert_eq!(palette.ink, "#161616");
        assert_eq!(palette.accent, "#8b6836");

        let path = std::env::temp_dir().join(format!("markdown-studio-dark-{}.pdf", std::process::id()));
        write_pdf(&path, &request).unwrap();
        let pdf = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
        let _ = fs::remove_file(path);
        assert!(!pdf.contains(" re f"), "pages must not paint a background");
        assert!(pdf.contains("0.086 0.086 0.086 rg"), "body text must use dark ink");

        let html = build_html(&request);
        assert!(html.contains("@media print{:root{--paper:transparent;--ink:#161616;"));
    }

    #[test]
    fn markdown_text_keeps_table_cells_and_nested_items_apart() {
        let lines = markdown_text("| Tool | Version |\n|---|---|\n| pnpm | 11.5.3 |\n\n- one\n  - two\n");
        assert_eq!(lines[0], "Tool | Version");
        assert_eq!(lines[1], "pnpm | 11.5.3");
        assert!(lines.contains(&"• one".to_string()));
        assert!(lines.contains(&"• two".to_string()));
    }

    #[test]
    fn pdf_text_encodes_typographic_punctuation() {
        assert_eq!(pdf_escape("1–5 — “ok” • é"), "1\\2265 \\227 \\223ok\\224 \\225 \\351");
        assert_eq!(pdf_escape("→"), "?");
    }

    #[test]
    fn pdf_text_stays_inside_the_margins_on_every_page() {
        for preset in ["narrow", "normal", "wide"] {
            let mut options = sample_options();
            options.margins = preset.into();
            let request = sample_request(options, vec![long_file()]);
            let margins = page_margins(preset);
            let pages = layout_pdf(&request);
            assert!(pages.len() > 3, "{preset}: expected several pages");
            for line in pages.iter().flatten() {
                let bold = line.style != PdfStyle::Body;
                let right = line.x + text_width(&line.text, line.size, bold);
                assert!(line.baseline - line.size * 0.25 >= margins.bottom * PT_PER_MM, "{preset}: {line:?}");
                assert!(line.baseline + line.size <= PDF_PAGE_HEIGHT - margins.top * PT_PER_MM + 0.01, "{preset}: {line:?}");
                assert!(line.x >= margins.left * PT_PER_MM - 0.01, "{preset}: {line:?}");
                assert!(right <= PDF_PAGE_WIDTH - margins.right * PT_PER_MM + 0.01, "{preset}: {line:?}");
            }
        }
    }

    #[test]
    fn pdf_numbers_every_page() {
        let mut options = sample_options();
        options.page_number_format = "page-of".into();
        let request = sample_request(options, vec![long_file()]);
        let path = std::env::temp_dir().join(format!("markdown-studio-{}.pdf", std::process::id()));
        write_pdf(&path, &request).unwrap();
        let pdf = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
        let _ = fs::remove_file(path);
        let pages = layout_pdf(&request).len();
        for page in 1..=pages {
            assert!(pdf.contains(&format!("(Page {page} of {pages}) Tj")), "missing page {page}");
        }
    }

    #[test]
    fn docx_footer_uses_page_fields_only_when_enabled() {
        let mut options = sample_options();
        options.page_number_format = "page-of".into();
        let request = sample_request(options.clone(), Vec::new());
        let (header, footer) = running_slots(&request.export_options);
        let footer_xml = word_header_footer("ftr", &footer, &request, 9360);
        assert!(footer_xml.contains("<w:ftr "));
        assert_eq!(footer_xml.matches(" PAGE ").count(), 1);
        assert_eq!(footer_xml.matches(" NUMPAGES ").count(), 1);
        assert!(footer_xml.contains(">Page </w:t>"));
        let header_xml = word_header_footer("hdr", &header, &request, 9360);
        assert!(header_xml.contains("<w:hdr ") && header_xml.contains("Field &quot;Notes&quot;"));

        options.page_numbers = false;
        let request = sample_request(options, Vec::new());
        let footer_xml = word_header_footer("ftr", &running_slots(&request.export_options).1, &request, 9360);
        assert!(!footer_xml.contains("PAGE"));
    }

    #[test]
    fn print_css_maps_tokens_to_page_counters() {
        let request = sample_request(sample_options(), Vec::new());
        assert_eq!(css_content("Page {page} of {pages}", &request), r#""Page " counter(page) " of " counter(pages)"#);
        assert_eq!(css_content("{title}", &request), r#""Field \"Notes\"""#);
        assert_eq!(css_content("</style>", &request), r#""\3c /style>""#);
        let css = print_page_css(&request);
        assert!(css.starts_with("@page{margin:25.4mm 25.4mm 25.4mm 25.4mm;"));
        assert!(css.contains("@bottom-center{content:counter(page);"));
        assert!(css.contains("@top-center{content:\"Field \\\"Notes\\\"\";"));
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

    #[test]
    fn accepts_markdown_file_arguments_only() {
        let directory = std::env::temp_dir();
        let paths = open_paths_from_arguments(
            vec![
                "--flag".to_string(),
                "notes.txt".to_string(),
                "chapter.md".to_string(),
                "appendix.markdown".to_string(),
            ],
            Some(&directory),
        );
        assert_eq!(
            paths,
            vec![
                directory.join("chapter.md"),
                directory.join("appendix.markdown")
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn accepts_windows_absolute_markdown_paths() {
        let path = r"C:\Users\Example\Documents\chapter.md";
        let paths = open_paths_from_arguments(vec![path.to_string()], None);
        assert_eq!(paths, vec![PathBuf::from(path)]);
    }
}

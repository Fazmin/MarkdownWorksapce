# Markdown Studio

**A smilple, minimalistic workspace for reading, organizing, and publishing Markdown.**

Markdown Studio turns a single Markdown file or an entire folder into a polished, cohesive document. Arrange sections, choose comfortable typography, and publish to PDF, DOCX, self-contained HTML, or one combined Markdown file.

## Why I built it

I enjoy writing in Markdown because it stays out of the way. Publishing that writing, however, often means reaching for a much larger tool or maintaining a collection of conversion scripts.

Markdown Studio is my attempt to make that final step feel as simple as the writing itself: open your files, put them in order, make them pleasant to read, and publish. It is intentionally focused—not a replacement for a full word processor, but a thoughtfully designed home for Markdown documents.

Everything stays on your device. There are no accounts, uploads, or external conversion programs.

## Highlights

- Read individual files or organize complete folders with drag-and-drop reordering
- Render GitHub-Flavored Markdown with syntax highlighting and in-document search
- Choose from six token-based themes and adjust typeface and font size
- Navigate long documents with an optional full-document minimap
- Save and reopen projects through a local SQLite-backed library
- Export to PDF, DOCX, self-contained HTML, or combined Markdown
- Print the current file or an entire collection
- Work comfortably in light or dark mode

## Opening files from the desktop

Packaged builds register Markdown Studio as a handler for `.md` and `.markdown` files. Use **Open with → Markdown Studio** from the system file manager to load a document in a clean quick-view layout. Use the **More** title-bar toggle when you want the organizer and settings sidebars. If the app is already running, the existing window is focused and receives the file instead of opening a second instance.

File associations are registered by the platform installer, so reinstall the app after association configuration changes during development.

## Getting started

### Prerequisites

- [Node.js 20+](https://nodejs.org/)
- [Rust stable](https://www.rust-lang.org/tools/install)
- The [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your operating system

### Run the desktop app

```sh
npm install
npm run desktop
```

For a browser-only preview of the interface, run:

```sh
npm run dev
```

Native file dialogs, SQLite persistence, and PDF and DOCX export are available only in the Tauri desktop app.

## Quality checks

```sh
npm run check
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

To create a production installer:

```sh
npm run tauri -- build
```

Tauri writes the resulting bundle to `src-tauri/target/release/bundle/`.

## Packaging

- **Windows:** Install Microsoft C++ Build Tools and WebView2, then run the production build command.
- **macOS:** Install Xcode Command Line Tools and build on macOS to produce an app bundle and DMG.
- **Linux:** Install the WebKitGTK and AppIndicator development packages listed in the Tauri prerequisites, then build on the target distribution.

Installers should be built on each target operating system.

## Built for substantial documents

Markdown Studio is designed to remain responsive with long documents and collections containing hundreds of files. Actual performance will vary with document complexity, file size, and hardware.

- Parsing, file imports, database operations, and exports run on Rust worker threads.
- Rendered documents use a bounded 12 MB, 16-entry cache with automatic eviction.
- File reads are batched, while multi-file print rendering uses limited concurrency.
- Syntax highlighting runs in short chunks and safely falls back to plain code for unusually large blocks.
- Search is debounced and limits DOM highlights to 500 matches; a `+` beside the result count indicates that the limit was reached.
- The minimap summarizes up to 320 structural blocks instead of duplicating the rendered document.
- Off-screen sections and file cards use browser rendering containment where supported.
- Indexed project metadata keeps the library responsive without loading every file body.
- SQLite uses WAL mode with normal synchronization for responsive local persistence.

The Rust test suite includes a 250-file import and a generated 20,000-paragraph document. These are regression checks rather than formal benchmarks.

## Architecture

- **Svelte** powers the reader, organizer, search, themes, and publishing controls.
- **Rust** parses GitHub-Flavored Markdown, imports folders, and handles export work.
- **SQLite** stores projects in the operating system's application-data directory.
- The PDF exporter is a compact, pure-Rust implementation with no browser or external binary.
- DOCX export writes the Open XML package directly in Rust.
- Shared theme tokens carry paper, ink, accent, sizing, and rhythm from the reader into exported documents.

---

Markdown Studio is built around a simple idea: your writing should remain yours, and publishing it should feel effortless.

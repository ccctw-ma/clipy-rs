mod clipboard;
#[cfg(target_os = "macos")]
mod gui;
mod sensitive;
mod storage;

use std::cmp::Reverse;
use std::env;
use std::io::{self, Read, Write};
use std::thread;
use std::time::Duration;

use crate::storage::{HistoryEntry, SnippetEntry, Store};

const DEFAULT_MAX_ITEMS: usize = 100;
const DEFAULT_INTERVAL_MS: u64 = 750;
const DEFAULT_MAX_BYTES: usize = 256 * 1024;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        args.push(default_command());
    }

    let store = Store::open_default()?;
    let command = args.remove(0);
    match command.as_str() {
        "capture" => cmd_capture(&store, &args),
        "watch" => cmd_watch(&store, &args),
        "list" | "ls" => cmd_list(&store, &args),
        "pick" => cmd_pick(&store, &args),
        "copy" => cmd_copy(&store, &args),
        "remove" | "rm" => cmd_remove(&store, &args),
        "pin" => cmd_pin(&store, &args, true),
        "unpin" => cmd_pin(&store, &args, false),
        "clear" => cmd_clear(&store, &args),
        #[cfg(target_os = "macos")]
        "gui" | "app" | "menubar" => gui::run(),
        #[cfg(not(target_os = "macos"))]
        "gui" | "app" | "menubar" => Err("the menu bar GUI is only available on macOS".to_string()),
        "snip" | "snippet" | "snippets" => cmd_snippet(&store, &args),
        "stats" => cmd_stats(&store),
        "path" => {
            println!("{}", store.root().display());
            Ok(())
        }
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        unknown => Err(format!("unknown command `{unknown}`. Run `clipy-rs help`.")),
    }
}

fn default_command() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(path) = env::current_exe()
            && path
                .components()
                .any(|component| component.as_os_str().to_string_lossy().ends_with(".app"))
        {
            return "gui".to_string();
        }
    }

    "help".to_string()
}

fn cmd_capture(store: &Store, args: &[String]) -> Result<(), String> {
    let allow_sensitive = has_flag(args, "--allow-sensitive");
    let max_items = parse_usize_flag(args, "--max-items")?.unwrap_or(DEFAULT_MAX_ITEMS);
    let max_bytes = parse_usize_flag(args, "--max-bytes")?.unwrap_or(DEFAULT_MAX_BYTES);
    let text = clipboard::read_text()?;
    let result = capture_text(store, text, allow_sensitive, max_items, max_bytes)?;
    println!("{result}");
    Ok(())
}

fn cmd_watch(store: &Store, args: &[String]) -> Result<(), String> {
    let allow_sensitive = has_flag(args, "--allow-sensitive");
    let max_items = parse_usize_flag(args, "--max-items")?.unwrap_or(DEFAULT_MAX_ITEMS);
    let max_bytes = parse_usize_flag(args, "--max-bytes")?.unwrap_or(DEFAULT_MAX_BYTES);
    let interval_ms = parse_u64_flag(args, "--interval-ms")?.unwrap_or(DEFAULT_INTERVAL_MS);
    if interval_ms < 100 {
        return Err("--interval-ms must be at least 100".to_string());
    }

    println!(
        "watching clipboard every {interval_ms}ms; history: {}",
        store.history_path().display()
    );
    println!("press Ctrl-C to stop");

    let mut last_seen = String::new();
    loop {
        match clipboard::read_text() {
            Ok(text) => {
                if text != last_seen {
                    last_seen = text.clone();
                    match capture_text(store, text, allow_sensitive, max_items, max_bytes) {
                        Ok(message) if !message.starts_with("ignored") => println!("{message}"),
                        Ok(_) => {}
                        Err(err) => eprintln!("capture failed: {err}"),
                    }
                }
            }
            Err(err) => eprintln!("read failed: {err}"),
        }
        thread::sleep(Duration::from_millis(interval_ms));
    }
}

fn cmd_list(store: &Store, args: &[String]) -> Result<(), String> {
    let full = has_flag(args, "--full");
    let limit = parse_usize_flag(args, "--limit")?;
    let query = free_text(args, &["--full", "--limit"]);
    let entries = store.load_history()?;
    let entries = filter_history(entries, query.as_deref());
    print_history(&entries, limit, full);
    Ok(())
}

fn cmd_pick(store: &Store, args: &[String]) -> Result<(), String> {
    let paste = has_flag(args, "--paste");
    let query = free_text(args, &["--paste"]);
    let entries = filter_history(store.load_history()?, query.as_deref());
    if entries.is_empty() {
        println!("no matching history items");
        return Ok(());
    }

    print_history(&entries, Some(20), false);
    let index = prompt_index("copy which item? ")?;
    let entry = entries
        .get(index.saturating_sub(1))
        .ok_or_else(|| format!("selection {index} is out of range"))?;
    copy_history_entry(store, entry.id, paste)
}

fn cmd_copy(store: &Store, args: &[String]) -> Result<(), String> {
    let paste = has_flag(args, "--paste");
    let use_id = has_flag(args, "--id");
    let key = first_positional(args, &["--paste", "--id"])
        .ok_or_else(|| "copy requires a 1-based list index, or --id <entry-id>".to_string())?;
    let entries = sorted_history(store.load_history()?);
    let entry = if use_id {
        let id = key
            .parse::<u64>()
            .map_err(|_| format!("invalid entry id `{key}`"))?;
        entries.iter().find(|entry| entry.id == id)
    } else {
        let index = key
            .parse::<usize>()
            .map_err(|_| format!("invalid list index `{key}`"))?;
        entries.get(index.saturating_sub(1))
    }
    .ok_or_else(|| format!("history item `{key}` was not found"))?;
    copy_history_entry(store, entry.id, paste)
}

fn cmd_remove(store: &Store, args: &[String]) -> Result<(), String> {
    let use_id = has_flag(args, "--id");
    let key = first_positional(args, &["--id"])
        .ok_or_else(|| "remove requires a 1-based list index, or --id <entry-id>".to_string())?;
    let mut entries = sorted_history(store.load_history()?);
    let id = resolve_history_id(&entries, key, use_id)?;
    let before = entries.len();
    entries.retain(|entry| entry.id != id);
    store.save_history(&entries)?;
    println!("removed {} item", before - entries.len());
    Ok(())
}

fn cmd_pin(store: &Store, args: &[String], pinned: bool) -> Result<(), String> {
    let use_id = has_flag(args, "--id");
    let key = first_positional(args, &["--id"]).ok_or_else(|| {
        if pinned {
            "pin requires a 1-based list index, or --id <entry-id>".to_string()
        } else {
            "unpin requires a 1-based list index, or --id <entry-id>".to_string()
        }
    })?;
    let mut entries = sorted_history(store.load_history()?);
    let id = resolve_history_id(&entries, key, use_id)?;
    let entry = entries
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("history item `{key}` was not found"))?;
    entry.pinned = pinned;
    store.save_history(&entries)?;
    println!("{} item {}", if pinned { "pinned" } else { "unpinned" }, id);
    Ok(())
}

fn cmd_clear(store: &Store, args: &[String]) -> Result<(), String> {
    if !has_flag(args, "--yes") {
        return Err("clear refuses to run without --yes".to_string());
    }
    store.save_history(&[])?;
    store.save_rich_history(&[])?;
    println!("cleared history");
    Ok(())
}

fn cmd_snippet(store: &Store, args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    if args.is_empty() {
        args.push("list".to_string());
    }
    let command = args.remove(0);
    match command.as_str() {
        "add" => snippet_add(store, &args),
        "save" | "from-clipboard" => snippet_save_clipboard(store, &args),
        "list" | "ls" => snippet_list(store, &args),
        "pick" => snippet_pick(store, &args),
        "copy" => snippet_copy(store, &args),
        "remove" | "rm" => snippet_remove(store, &args),
        unknown => Err(format!("unknown snippet command `{unknown}`")),
    }
}

fn snippet_add(store: &Store, args: &[String]) -> Result<(), String> {
    let name = args
        .first()
        .ok_or_else(|| "snip add requires <name> <text>".to_string())?;
    let text = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        read_stdin_to_string()?
    };
    if name.trim().is_empty() {
        return Err("snippet name cannot be empty".to_string());
    }
    if text.is_empty() {
        return Err("snippet text cannot be empty".to_string());
    }

    let mut snippets = store.load_snippets()?;
    storage::upsert_snippet(&mut snippets, name.to_string(), text);
    store.save_snippets(&snippets)?;
    println!("saved snippet `{name}`");
    Ok(())
}

fn snippet_save_clipboard(store: &Store, args: &[String]) -> Result<(), String> {
    let name = first_positional(args, &[])
        .ok_or_else(|| "snip save requires <name>".to_string())?
        .trim();
    if name.is_empty() {
        return Err("snippet name cannot be empty".to_string());
    }

    let text = clipboard::read_text()?;
    if text.is_empty() {
        return Err("clipboard text is empty".to_string());
    }

    let mut snippets = store.load_snippets()?;
    storage::upsert_snippet(&mut snippets, name.to_string(), text);
    store.save_snippets(&snippets)?;
    println!("saved clipboard as snippet `{name}`");
    Ok(())
}

fn snippet_list(store: &Store, args: &[String]) -> Result<(), String> {
    let query = free_text(args, &["--limit"]);
    let limit = parse_usize_flag(args, "--limit")?;
    let snippets = filter_snippets(store.load_snippets()?, query.as_deref());
    if snippets.is_empty() {
        println!("no snippets");
        return Ok(());
    }
    print_snippets(&snippets, limit);
    Ok(())
}

fn snippet_pick(store: &Store, args: &[String]) -> Result<(), String> {
    let paste = has_flag(args, "--paste");
    let query = free_text(args, &["--paste"]);
    let snippets = filter_snippets(store.load_snippets()?, query.as_deref());
    if snippets.is_empty() {
        println!("no matching snippets");
        return Ok(());
    }

    print_snippets(&snippets, Some(20));
    let index = prompt_index("copy which snippet? ")?;
    let snippet = snippets
        .get(index.saturating_sub(1))
        .ok_or_else(|| format!("selection {index} is out of range"))?;
    copy_snippet_by_name(store, &snippet.name, paste)?;
    println!("copied snippet `{}`", snippet.name);
    Ok(())
}

fn print_snippets(snippets: &[SnippetEntry], limit: Option<usize>) {
    for (idx, snippet) in snippets
        .iter()
        .take(limit.unwrap_or(usize::MAX))
        .enumerate()
    {
        println!(
            "{:>3}. {:<24} uses={:<3} {}",
            idx + 1,
            snippet.name,
            snippet.use_count,
            preview(&snippet.content, 80)
        );
    }
}

fn snippet_copy(store: &Store, args: &[String]) -> Result<(), String> {
    let paste = has_flag(args, "--paste");
    let key = first_positional(args, &["--paste"])
        .ok_or_else(|| "snip copy requires a name, list index, or search query".to_string())?;
    let snippets = sorted_snippets(store.load_snippets()?);
    let snippet = resolve_snippet(&snippets, key)?;
    copy_snippet_by_name(store, &snippet.name, paste)?;
    println!("copied snippet `{}`", snippet.name);
    Ok(())
}

fn snippet_remove(store: &Store, args: &[String]) -> Result<(), String> {
    let name = args
        .first()
        .ok_or_else(|| "snip remove requires <name>".to_string())?;
    let mut snippets = store.load_snippets()?;
    let before = snippets.len();
    snippets.retain(|snippet| snippet.name != *name);
    store.save_snippets(&snippets)?;
    println!("removed {} snippet", before - snippets.len());
    Ok(())
}

fn cmd_stats(store: &Store) -> Result<(), String> {
    let history = store.load_history()?;
    let rich_history = store.load_rich_history()?;
    let snippets = store.load_snippets()?;
    let bytes: usize = history.iter().map(|entry| entry.content.len()).sum();
    println!("history items: {}", history.len());
    println!("image/file history items: {}", rich_history.len());
    println!("history bytes: {bytes}");
    println!("snippets: {}", snippets.len());
    println!("data dir: {}", store.root().display());
    Ok(())
}

fn capture_text(
    store: &Store,
    text: String,
    allow_sensitive: bool,
    max_items: usize,
    max_bytes: usize,
) -> Result<String, String> {
    let text = normalize_clipboard_text(text);
    if text.is_empty() {
        return Ok("ignored empty clipboard".to_string());
    }
    if text.len() > max_bytes {
        return Ok(format!(
            "ignored clipboard item larger than {max_bytes} bytes ({})",
            text.len()
        ));
    }
    if !allow_sensitive && sensitive::looks_sensitive(&text) {
        return Ok(
            "skipped sensitive-looking clipboard item; pass --allow-sensitive to keep it"
                .to_string(),
        );
    }

    let captured_len = text.len();
    let mut entries = store.load_history()?;
    let inserted = storage::upsert_history(&mut entries, text);
    storage::prune_history(&mut entries, max_items);
    store.save_history(&entries)?;
    if inserted {
        Ok(format!(
            "captured new clipboard item ({})",
            human_bytes(captured_len)
        ))
    } else {
        Ok("updated existing clipboard item".to_string())
    }
}

fn copy_history_entry(store: &Store, id: u64, paste: bool) -> Result<(), String> {
    let mut entries = store.load_history()?;
    let entry_index = entries
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| format!("history item `{id}` was not found"))?;
    clipboard::write_text(&entries[entry_index].content)?;
    if paste {
        clipboard::paste_frontmost()?;
    }
    entries[entry_index].use_count += 1;
    entries[entry_index].updated_at = storage::now_millis();
    store.save_history(&entries)?;
    println!("copied history item {id}");
    Ok(())
}

fn copy_snippet_by_name(store: &Store, name: &str, paste: bool) -> Result<(), String> {
    let mut snippets = store.load_snippets()?;
    let snippet_index = snippets
        .iter()
        .position(|snippet| snippet.name == name)
        .ok_or_else(|| format!("snippet `{name}` was not found"))?;
    let rendered = render_snippet_content(&snippets[snippet_index].content)?;
    clipboard::write_text(&rendered.content)?;
    if paste {
        clipboard::paste_frontmost()?;
        clipboard::move_cursor_left(rendered.cursor_left)?;
    }
    snippets[snippet_index].use_count += 1;
    snippets[snippet_index].updated_at = storage::now_millis();
    store.save_snippets(&snippets)?;
    Ok(())
}

fn sorted_history(mut entries: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    entries.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    entries
}

fn sorted_snippets(mut snippets: Vec<SnippetEntry>) -> Vec<SnippetEntry> {
    snippets.sort_by_key(|snippet| Reverse(snippet.updated_at));
    snippets
}

fn filter_snippets(snippets: Vec<SnippetEntry>, query: Option<&str>) -> Vec<SnippetEntry> {
    let mut snippets = sorted_snippets(snippets);
    if let Some(query) = query {
        let needle = query.to_lowercase();
        snippets.retain(|snippet| snippet_matches_query(snippet, &needle));
    }
    snippets
}

fn resolve_snippet<'a>(
    snippets: &'a [SnippetEntry],
    key: &str,
) -> Result<&'a SnippetEntry, String> {
    if let Ok(index) = key.parse::<usize>()
        && index > 0
        && let Some(snippet) = snippets.get(index - 1)
    {
        return Ok(snippet);
    }

    if let Some(snippet) = snippets.iter().find(|snippet| snippet.name == key) {
        return Ok(snippet);
    }

    let leaf_matches = snippets
        .iter()
        .filter(|snippet| snippet_display_name(&snippet.name) == key)
        .collect::<Vec<_>>();
    match leaf_matches.as_slice() {
        [snippet] => return Ok(*snippet),
        [] => {}
        _ => {
            return Err(format!(
                "snippet `{key}` matched {} folders; use the full name or copy by index",
                leaf_matches.len()
            ));
        }
    }

    let needle = key.to_lowercase();
    let matches = snippets
        .iter()
        .filter(|snippet| snippet_matches_query(snippet, &needle))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [snippet] => Ok(*snippet),
        [] => Err(format!("snippet `{key}` was not found")),
        _ => Err(format!(
            "snippet `{key}` matched {} items; use `snip list {key}` and copy by index",
            matches.len()
        )),
    }
}

fn snippet_matches_query(snippet: &SnippetEntry, needle: &str) -> bool {
    snippet.name.to_lowercase().contains(needle)
        || snippet_display_name(&snippet.name)
            .to_lowercase()
            .contains(needle)
        || snippet.content.to_lowercase().contains(needle)
}

fn snippet_display_name(name: &str) -> &str {
    let name = name.trim_matches('/');
    name.rsplit('/').next().unwrap_or(name)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenderedSnippet {
    content: String,
    cursor_left: usize,
}

fn render_snippet_content(content: &str) -> Result<RenderedSnippet, String> {
    let clipboard_text = if content.contains("{{clipboard}}") {
        Some(clipboard::read_text()?)
    } else {
        None
    };
    Ok(render_snippet_content_with_clipboard(
        content,
        clipboard_text.as_deref(),
    ))
}

fn render_snippet_content_with_clipboard(
    content: &str,
    clipboard_text: Option<&str>,
) -> RenderedSnippet {
    let mut content = content.replace("{{clipboard}}", clipboard_text.unwrap_or_default());
    let cursor_marker = earliest_marker(&content, &["{{cursor}}", "$|$"]);
    let cursor_left = cursor_marker
        .map(|(index, marker)| content[index + marker.len()..].chars().count())
        .unwrap_or(0);
    content = content.replace("{{cursor}}", "").replace("$|$", "");
    RenderedSnippet {
        content,
        cursor_left,
    }
}

fn earliest_marker<'a>(text: &str, markers: &[&'a str]) -> Option<(usize, &'a str)> {
    markers
        .iter()
        .filter_map(|marker| text.find(marker).map(|index| (index, *marker)))
        .min_by_key(|(index, _)| *index)
}

fn filter_history(entries: Vec<HistoryEntry>, query: Option<&str>) -> Vec<HistoryEntry> {
    let mut entries = sorted_history(entries);
    if let Some(query) = query {
        let needle = query.to_lowercase();
        entries.retain(|entry| entry.content.to_lowercase().contains(&needle));
    }
    entries
}

fn print_history(entries: &[HistoryEntry], limit: Option<usize>, full: bool) {
    if entries.is_empty() {
        println!("no history");
        return;
    }
    for (idx, entry) in entries.iter().take(limit.unwrap_or(usize::MAX)).enumerate() {
        let pin = if entry.pinned { "*" } else { " " };
        if full {
            println!(
                "{:>3}.{} id={} uses={} bytes={} updated={}",
                idx + 1,
                pin,
                entry.id,
                entry.use_count,
                entry.content.len(),
                entry.updated_at
            );
            println!("{}", entry.content);
            println!();
        } else {
            println!(
                "{:>3}.{} id={} {:>8} {}",
                idx + 1,
                pin,
                entry.id,
                human_bytes(entry.content.len()),
                preview(&entry.content, 90)
            );
        }
    }
}

fn resolve_history_id(entries: &[HistoryEntry], key: &str, use_id: bool) -> Result<u64, String> {
    if use_id {
        let id = key
            .parse::<u64>()
            .map_err(|_| format!("invalid entry id `{key}`"))?;
        if entries.iter().any(|entry| entry.id == id) {
            Ok(id)
        } else {
            Err(format!("history item `{key}` was not found"))
        }
    } else {
        let index = key
            .parse::<usize>()
            .map_err(|_| format!("invalid list index `{key}`"))?;
        entries
            .get(index.saturating_sub(1))
            .map(|entry| entry.id)
            .ok_or_else(|| format!("history index `{key}` was not found"))
    }
}

fn normalize_clipboard_text(text: String) -> String {
    text.trim_matches('\0').to_string()
}

fn preview(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(text.len()));
    let mut previous_space = false;
    for ch in text.chars() {
        let mapped = if ch.is_control() || ch.is_whitespace() {
            ' '
        } else {
            ch
        };
        if mapped == ' ' {
            if previous_space {
                continue;
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
        out.push(mapped);
        if out.chars().count() >= max_chars {
            out.push_str("...");
            break;
        }
    }
    out
}

fn human_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / 1024.0 / 1024.0)
    }
}

fn prompt_index(prompt: &str) -> Result<usize, String> {
    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush stdout: {err}"))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|err| format!("failed to read selection: {err}"))?;
    line.trim()
        .parse::<usize>()
        .map_err(|_| format!("invalid selection `{}`", line.trim()))
}

fn read_stdin_to_string() -> Result<String, String> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|err| format!("failed to read stdin: {err}"))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_string())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn parse_usize_flag(args: &[String], flag: &str) -> Result<Option<usize>, String> {
    parse_flag_value(args, flag).map(|value| {
        value
            .map(|raw| {
                raw.parse::<usize>()
                    .map_err(|_| format!("invalid {flag} value `{raw}`"))
            })
            .transpose()
    })?
}

fn parse_u64_flag(args: &[String], flag: &str) -> Result<Option<u64>, String> {
    parse_flag_value(args, flag).map(|value| {
        value
            .map(|raw| {
                raw.parse::<u64>()
                    .map_err(|_| format!("invalid {flag} value `{raw}`"))
            })
            .transpose()
    })?
}

fn parse_flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, String> {
    for (idx, arg) in args.iter().enumerate() {
        if arg == flag {
            return args
                .get(idx + 1)
                .map(|value| Some(value.as_str()))
                .ok_or_else(|| format!("{flag} requires a value"));
        }
        if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn first_positional<'a>(args: &'a [String], flags: &[&str]) -> Option<&'a str> {
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if flags.contains(&arg.as_str()) {
            if flag_takes_value(arg) {
                skip_next = true;
            }
            continue;
        }
        if flags
            .iter()
            .any(|flag| arg.starts_with(&format!("{flag}=")))
        {
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        return Some(arg);
    }
    None
}

fn free_text(args: &[String], flags: &[&str]) -> Option<String> {
    let mut words = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if flags.contains(&arg.as_str()) {
            if flag_takes_value(arg) {
                skip_next = true;
            }
            continue;
        }
        if flags
            .iter()
            .any(|flag| arg.starts_with(&format!("{flag}=")))
        {
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        words.push(arg.as_str());
    }
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

fn flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "--limit" | "--max-items" | "--max-bytes" | "--interval-ms"
    )
}

fn print_help() {
    println!(
        r#"clipy-rs - small macOS clipboard history tool

USAGE
  clipy-rs capture [--allow-sensitive] [--max-items N] [--max-bytes N]
  clipy-rs watch [--interval-ms N] [--allow-sensitive]
  clipy-rs list [query] [--limit N] [--full]
  clipy-rs pick [query] [--paste]
  clipy-rs gui
  clipy-rs copy <index> [--paste]
  clipy-rs copy --id <entry-id> [--paste]
  clipy-rs pin|unpin <index>
  clipy-rs remove <index>
  clipy-rs clear --yes
  clipy-rs snip add <name> <text>
  clipy-rs snip save <name>
  clipy-rs snip list [query]
  clipy-rs snip pick [query] [--paste]
  clipy-rs snip copy <name|index|query> [--paste]
  clipy-rs snip remove <name>
  clipy-rs stats
  clipy-rs path

NOTES
  - Data is stored locally under ~/Library/Application Support/clipy-rs.
  - watch/capture skip obvious secrets by default; pass --allow-sensitive to store them.
  - Snippet names can use folders like work/signature; snippets support {{clipboard}} and {{cursor}}.
  - --paste posts Cmd+V through macOS Accessibility and may need system permission.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_collapses_whitespace() {
        assert_eq!(preview("hello\n\nworld\t!", 80), "hello world !");
    }

    #[test]
    fn free_text_ignores_flag_values() {
        let args = vec![
            "hello".to_string(),
            "--limit".to_string(),
            "5".to_string(),
            "world".to_string(),
        ];
        assert_eq!(
            free_text(&args, &["--limit"]),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn resolves_snippet_by_index_exact_name_and_unique_query() {
        let snippets = vec![
            SnippetEntry {
                name: "work/signature".to_string(),
                content: "Regards".to_string(),
                created_at: 1,
                updated_at: 3,
                use_count: 0,
            },
            SnippetEntry {
                name: "email".to_string(),
                content: "hello@example.com".to_string(),
                created_at: 1,
                updated_at: 2,
                use_count: 0,
            },
        ];

        assert_eq!(
            resolve_snippet(&snippets, "1").unwrap().name,
            "work/signature"
        );
        assert_eq!(
            resolve_snippet(&snippets, "signature").unwrap().name,
            "work/signature"
        );
        assert_eq!(resolve_snippet(&snippets, "hello@").unwrap().name, "email");
    }

    #[test]
    fn renders_snippet_clipboard_and_cursor_markers() {
        let rendered =
            render_snippet_content_with_clipboard("wrap({{clipboard}}){{cursor}};", Some("value"));

        assert_eq!(rendered.content, "wrap(value);");
        assert_eq!(rendered.cursor_left, 1);
    }
}

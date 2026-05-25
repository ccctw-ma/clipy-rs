use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_MAGIC: &[u8] = b"CLIPY_RS_HISTORY_V1\n";
const SNIPPET_MAGIC: &[u8] = b"CLIPY_RS_SNIPPET_V1\n";
const RICH_HISTORY_MAGIC: &[u8] = b"CLIPY_RS_RICH_HISTORY_V1\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub id: u64,
    pub content: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub use_count: u64,
    pub pinned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnippetEntry {
    pub name: String,
    pub content: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub use_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichClipboardFlavor {
    pub type_name: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RichClipboardKind {
    Image,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichHistoryEntry {
    pub id: u64,
    pub kind: RichClipboardKind,
    pub label: String,
    pub flavors: Vec<RichClipboardFlavor>,
    pub created_at: u64,
    pub updated_at: u64,
    pub use_count: u64,
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    English,
    Chinese,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppSettings {
    pub language: Language,
    pub capture_rich_clipboard: bool,
    pub max_history_items: usize,
    pub visible_history_items: usize,
    pub menu_width: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: Language::English,
            capture_rich_clipboard: true,
            max_history_items: 40,
            visible_history_items: 10,
            menu_width: 260,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open_default() -> Result<Self, String> {
        let root = default_root()?;
        fs::create_dir_all(&root)
            .map_err(|err| format!("failed to create {}: {err}", root.display()))?;
        Ok(Self { root })
    }

    #[cfg(test)]
    pub fn open_at(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root)
            .map_err(|err| format!("failed to create {}: {err}", root.display()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn history_path(&self) -> PathBuf {
        self.root.join("history.bin")
    }

    fn snippets_path(&self) -> PathBuf {
        self.root.join("snippets.bin")
    }

    pub fn rich_history_path(&self) -> PathBuf {
        self.root.join("rich_history.bin")
    }

    fn settings_path(&self) -> PathBuf {
        self.root.join("settings.conf")
    }

    pub fn load_history(&self) -> Result<Vec<HistoryEntry>, String> {
        let path = self.history_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes =
            fs::read(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        decode_history(&bytes).map_err(|err| format!("failed to parse {}: {err}", path.display()))
    }

    pub fn save_history(&self, entries: &[HistoryEntry]) -> Result<(), String> {
        let path = self.history_path();
        write_atomic(&path, &encode_history(entries)?)
    }

    pub fn load_snippets(&self) -> Result<Vec<SnippetEntry>, String> {
        let path = self.snippets_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes =
            fs::read(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        decode_snippets(&bytes).map_err(|err| format!("failed to parse {}: {err}", path.display()))
    }

    pub fn save_snippets(&self, snippets: &[SnippetEntry]) -> Result<(), String> {
        let path = self.snippets_path();
        write_atomic(&path, &encode_snippets(snippets)?)
    }

    pub fn load_rich_history(&self) -> Result<Vec<RichHistoryEntry>, String> {
        let path = self.rich_history_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes =
            fs::read(&path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        decode_rich_history(&bytes)
            .map_err(|err| format!("failed to parse {}: {err}", path.display()))
    }

    pub fn save_rich_history(&self, entries: &[RichHistoryEntry]) -> Result<(), String> {
        let path = self.rich_history_path();
        write_atomic(&path, &encode_rich_history(entries)?)
    }

    pub fn load_settings(&self) -> Result<AppSettings, String> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(AppSettings::default());
        }
        let text = fs::read_to_string(&path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        Ok(decode_settings(&text))
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        let path = self.settings_path();
        let settings = normalize_settings(settings.clone());
        write_atomic(&path, encode_settings(&settings).as_bytes())
    }
}

pub fn upsert_history(entries: &mut Vec<HistoryEntry>, content: String) -> bool {
    let now = now_millis();
    if let Some(index) = entries.iter().position(|entry| entry.content == content) {
        let mut entry = entries.remove(index);
        entry.updated_at = now;
        entries.insert(0, entry);
        false
    } else {
        let next_id = entries
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        entries.insert(
            0,
            HistoryEntry {
                id: next_id,
                content,
                created_at: now,
                updated_at: now,
                use_count: 0,
                pinned: false,
            },
        );
        true
    }
}

pub fn prune_history(entries: &mut Vec<HistoryEntry>, max_items: usize) {
    if max_items == 0 {
        entries.retain(|entry| entry.pinned);
        return;
    }
    while entries.len() > max_items {
        if let Some(index) = entries.iter().rposition(|entry| !entry.pinned) {
            entries.remove(index);
        } else {
            entries.pop();
        }
    }
}

pub fn upsert_rich_history(
    entries: &mut Vec<RichHistoryEntry>,
    mut entry: RichHistoryEntry,
) -> bool {
    let now = now_millis();
    if let Some(index) = entries
        .iter()
        .position(|existing| rich_payload_eq(existing, &entry))
    {
        let mut existing = entries.remove(index);
        existing.label = entry.label;
        existing.flavors = entry.flavors;
        existing.updated_at = now;
        entries.insert(0, existing);
        false
    } else {
        let next_id = entries
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        entry.id = next_id;
        entry.created_at = now;
        entry.updated_at = now;
        entries.insert(0, entry);
        true
    }
}

pub fn prune_rich_history(entries: &mut Vec<RichHistoryEntry>, max_items: usize) {
    if max_items == 0 {
        entries.retain(|entry| entry.pinned);
        return;
    }
    while entries.len() > max_items {
        if let Some(index) = entries.iter().rposition(|entry| !entry.pinned) {
            entries.remove(index);
        } else {
            entries.pop();
        }
    }
}

fn rich_payload_eq(left: &RichHistoryEntry, right: &RichHistoryEntry) -> bool {
    left.kind == right.kind
        && left.flavors.len() == right.flavors.len()
        && left
            .flavors
            .iter()
            .zip(right.flavors.iter())
            .all(|(left, right)| left.type_name == right.type_name && left.data == right.data)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn default_root() -> Result<PathBuf, String> {
    if let Some(root) = env::var_os("RCLIPY_HOME") {
        return Ok(PathBuf::from(root));
    }
    let home = env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let home = PathBuf::from(home);
    if cfg!(target_os = "macos") {
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("clipy-rs"))
    } else {
        Ok(home.join(".local").join("share").join("clipy-rs"))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| format!("failed to write {}: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| {
        format!(
            "failed to replace {} with {}: {err}",
            path.display(),
            tmp.display()
        )
    })
}

fn encode_history(entries: &[HistoryEntry]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(HISTORY_MAGIC);
    write_u64(&mut bytes, entries.len() as u64);
    for entry in entries {
        write_u64(&mut bytes, entry.id);
        write_u64(&mut bytes, entry.created_at);
        write_u64(&mut bytes, entry.updated_at);
        write_u64(&mut bytes, entry.use_count);
        bytes.push(u8::from(entry.pinned));
        write_string(&mut bytes, &entry.content)?;
    }
    Ok(bytes)
}

fn decode_history(bytes: &[u8]) -> Result<Vec<HistoryEntry>, String> {
    if !bytes.starts_with(HISTORY_MAGIC) {
        return Err("bad history header".to_string());
    }
    let mut cursor = Cursor::new(&bytes[HISTORY_MAGIC.len()..]);
    let count = read_u64(&mut cursor)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = read_u64(&mut cursor)?;
        let created_at = read_u64(&mut cursor)?;
        let updated_at = read_u64(&mut cursor)?;
        let use_count = read_u64(&mut cursor)?;
        let pinned = read_bool(&mut cursor)?;
        let content = read_string(&mut cursor)?;
        entries.push(HistoryEntry {
            id,
            content,
            created_at,
            updated_at,
            use_count,
            pinned,
        });
    }
    Ok(entries)
}

fn encode_snippets(snippets: &[SnippetEntry]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(SNIPPET_MAGIC);
    write_u64(&mut bytes, snippets.len() as u64);
    for snippet in snippets {
        write_string(&mut bytes, &snippet.name)?;
        write_string(&mut bytes, &snippet.content)?;
        write_u64(&mut bytes, snippet.created_at);
        write_u64(&mut bytes, snippet.updated_at);
        write_u64(&mut bytes, snippet.use_count);
    }
    Ok(bytes)
}

fn decode_snippets(bytes: &[u8]) -> Result<Vec<SnippetEntry>, String> {
    if !bytes.starts_with(SNIPPET_MAGIC) {
        return Err("bad snippets header".to_string());
    }
    let mut cursor = Cursor::new(&bytes[SNIPPET_MAGIC.len()..]);
    let count = read_u64(&mut cursor)? as usize;
    let mut snippets = Vec::with_capacity(count);
    for _ in 0..count {
        let name = read_string(&mut cursor)?;
        let content = read_string(&mut cursor)?;
        let created_at = read_u64(&mut cursor)?;
        let updated_at = read_u64(&mut cursor)?;
        let use_count = read_u64(&mut cursor)?;
        snippets.push(SnippetEntry {
            name,
            content,
            created_at,
            updated_at,
            use_count,
        });
    }
    Ok(snippets)
}

fn encode_rich_history(entries: &[RichHistoryEntry]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(RICH_HISTORY_MAGIC);
    write_u64(&mut bytes, entries.len() as u64);
    for entry in entries {
        write_u64(&mut bytes, entry.id);
        bytes.push(match entry.kind {
            RichClipboardKind::Image => 1,
            RichClipboardKind::File => 2,
        });
        write_string(&mut bytes, &entry.label)?;
        write_u64(&mut bytes, entry.created_at);
        write_u64(&mut bytes, entry.updated_at);
        write_u64(&mut bytes, entry.use_count);
        bytes.push(u8::from(entry.pinned));
        write_u64(&mut bytes, entry.flavors.len() as u64);
        for flavor in &entry.flavors {
            write_string(&mut bytes, &flavor.type_name)?;
            write_bytes(&mut bytes, &flavor.data);
        }
    }
    Ok(bytes)
}

fn decode_rich_history(bytes: &[u8]) -> Result<Vec<RichHistoryEntry>, String> {
    if !bytes.starts_with(RICH_HISTORY_MAGIC) {
        return Err("bad rich history header".to_string());
    }
    let mut cursor = Cursor::new(&bytes[RICH_HISTORY_MAGIC.len()..]);
    let count = read_u64(&mut cursor)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = read_u64(&mut cursor)?;
        let kind = match read_byte(&mut cursor)? {
            1 => RichClipboardKind::Image,
            2 => RichClipboardKind::File,
            raw => return Err(format!("unknown rich clipboard kind {raw}")),
        };
        let label = read_string(&mut cursor)?;
        let created_at = read_u64(&mut cursor)?;
        let updated_at = read_u64(&mut cursor)?;
        let use_count = read_u64(&mut cursor)?;
        let pinned = read_bool(&mut cursor)?;
        let flavor_count = read_u64(&mut cursor)? as usize;
        let mut flavors = Vec::with_capacity(flavor_count);
        for _ in 0..flavor_count {
            flavors.push(RichClipboardFlavor {
                type_name: read_string(&mut cursor)?,
                data: read_bytes(&mut cursor)?,
            });
        }
        entries.push(RichHistoryEntry {
            id,
            kind,
            label,
            flavors,
            created_at,
            updated_at,
            use_count,
            pinned,
        });
    }
    Ok(entries)
}

fn encode_settings(settings: &AppSettings) -> String {
    let language = match settings.language {
        Language::English => "en",
        Language::Chinese => "zh-CN",
    };
    format!(
        "language={language}\ncapture_rich_clipboard={}\nmax_history_items={}\nvisible_history_items={}\nmenu_width={}\n",
        settings.capture_rich_clipboard,
        settings.max_history_items,
        settings.visible_history_items,
        settings.menu_width,
    )
}

fn decode_settings(text: &str) -> AppSettings {
    let mut settings = AppSettings::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "language" => {
                settings.language = match value.trim() {
                    "zh" | "zh-CN" | "chinese" => Language::Chinese,
                    _ => Language::English,
                };
            }
            "capture_rich_clipboard" => {
                settings.capture_rich_clipboard = matches!(value.trim(), "1" | "true" | "yes");
            }
            "max_history_items" => {
                if let Ok(parsed) = value.trim().parse::<usize>() {
                    settings.max_history_items = parsed;
                }
            }
            "visible_history_items" => {
                if let Ok(parsed) = value.trim().parse::<usize>() {
                    settings.visible_history_items = parsed;
                }
            }
            "menu_width" => {
                if let Ok(parsed) = value.trim().parse::<usize>() {
                    settings.menu_width = parsed;
                }
            }
            _ => {}
        }
    }
    normalize_settings(settings)
}

pub fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    settings.max_history_items = settings.max_history_items.clamp(1, 500);
    settings.visible_history_items = settings.visible_history_items.clamp(1, 100);
    if settings.visible_history_items > settings.max_history_items {
        settings.visible_history_items = settings.max_history_items;
    }
    settings.menu_width = settings.menu_width.clamp(180, 600);
    settings
}

fn write_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, String> {
    let mut bytes = [0; 8];
    cursor
        .read_exact(&mut bytes)
        .map_err(|err| format!("expected u64: {err}"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_bool(cursor: &mut Cursor<&[u8]>) -> Result<bool, String> {
    Ok(read_byte(cursor)? != 0)
}

fn read_byte(cursor: &mut Cursor<&[u8]>) -> Result<u8, String> {
    let mut byte = [0; 1];
    cursor
        .read_exact(&mut byte)
        .map_err(|err| format!("expected byte: {err}"))?;
    Ok(byte[0])
}

fn write_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let len = value.len();
    write_u64(bytes, len as u64);
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

fn read_string(cursor: &mut Cursor<&[u8]>) -> Result<String, String> {
    let len = read_u64(cursor)? as usize;
    let mut bytes = vec![0; len];
    cursor
        .read_exact(&mut bytes)
        .map_err(|err| format!("expected {len} bytes: {err}"))?;
    String::from_utf8(bytes).map_err(|err| format!("invalid utf8: {err}"))
}

fn write_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    write_u64(bytes, value.len() as u64);
    bytes.extend_from_slice(value);
}

fn read_bytes(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u8>, String> {
    let len = read_u64(cursor)? as usize;
    let mut bytes = vec![0; len];
    cursor
        .read_exact(&mut bytes)
        .map_err(|err| format!("expected {len} bytes: {err}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_roundtrip() {
        let entries = vec![HistoryEntry {
            id: 7,
            content: "hello\nworld".to_string(),
            created_at: 1,
            updated_at: 2,
            use_count: 3,
            pinned: true,
        }];

        let bytes = encode_history(&entries).unwrap();
        assert_eq!(decode_history(&bytes).unwrap(), entries);
    }

    #[test]
    fn snippet_roundtrip() {
        let snippets = vec![SnippetEntry {
            name: "sig".to_string(),
            content: "thanks".to_string(),
            created_at: 1,
            updated_at: 2,
            use_count: 3,
        }];

        let bytes = encode_snippets(&snippets).unwrap();
        assert_eq!(decode_snippets(&bytes).unwrap(), snippets);
    }

    #[test]
    fn rich_history_roundtrip() {
        let entries = vec![RichHistoryEntry {
            id: 9,
            kind: RichClipboardKind::Image,
            label: "PNG image".to_string(),
            flavors: vec![RichClipboardFlavor {
                type_name: "public.png".to_string(),
                data: vec![1, 2, 3],
            }],
            created_at: 1,
            updated_at: 2,
            use_count: 3,
            pinned: false,
        }];

        let bytes = encode_rich_history(&entries).unwrap();
        assert_eq!(decode_rich_history(&bytes).unwrap(), entries);
    }

    #[test]
    fn settings_roundtrip() {
        let settings = AppSettings {
            language: Language::Chinese,
            capture_rich_clipboard: false,
            max_history_items: 60,
            visible_history_items: 15,
            menu_width: 280,
        };

        assert_eq!(decode_settings(&encode_settings(&settings)), settings);
    }

    #[test]
    fn settings_are_normalized() {
        let settings = decode_settings(
            "language=en\ncapture_rich_clipboard=true\nmax_history_items=0\nvisible_history_items=999\nmenu_width=9999\n",
        );

        assert_eq!(settings.max_history_items, 1);
        assert_eq!(settings.visible_history_items, 1);
        assert_eq!(settings.menu_width, 600);
    }

    #[test]
    fn upsert_moves_duplicates_to_front() {
        let mut entries = Vec::new();
        assert!(upsert_history(&mut entries, "a".to_string()));
        assert!(upsert_history(&mut entries, "b".to_string()));
        assert!(!upsert_history(&mut entries, "a".to_string()));

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "a");
    }

    #[test]
    fn upsert_snippet_updates_existing_name() {
        let mut snippets = Vec::new();
        assert!(upsert_snippet(
            &mut snippets,
            "work/signature".to_string(),
            "old".to_string()
        ));
        assert!(!upsert_snippet(
            &mut snippets,
            "work/signature".to_string(),
            "new".to_string()
        ));

        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].content, "new");
    }

    #[test]
    fn pruning_keeps_pinned_items() {
        let mut entries = vec![
            entry(1, "a", false),
            entry(2, "b", true),
            entry(3, "c", false),
        ];
        prune_history(&mut entries, 1);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "b");
    }

    #[test]
    fn store_persists_history() {
        let root = std::env::temp_dir().join(format!(
            "clipy-rs-storage-test-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let store = Store::open_at(root.clone()).unwrap();
        let entries = vec![entry(1, "persisted", false)];

        store.save_history(&entries).unwrap();
        assert_eq!(store.load_history().unwrap(), entries);

        let _ = fs::remove_dir_all(root);
    }

    fn entry(id: u64, content: &str, pinned: bool) -> HistoryEntry {
        HistoryEntry {
            id,
            content: content.to_string(),
            created_at: id,
            updated_at: id,
            use_count: 0,
            pinned,
        }
    }
}

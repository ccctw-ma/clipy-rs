use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_MAGIC: &[u8] = b"CLIPY_RS_HISTORY_V1\n";
const SNIPPET_MAGIC: &[u8] = b"CLIPY_RS_SNIPPET_V1\n";

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
    let mut byte = [0; 1];
    cursor
        .read_exact(&mut byte)
        .map_err(|err| format!("expected bool: {err}"))?;
    Ok(byte[0] != 0)
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
    fn upsert_moves_duplicates_to_front() {
        let mut entries = Vec::new();
        assert!(upsert_history(&mut entries, "a".to_string()));
        assert!(upsert_history(&mut entries, "b".to_string()));
        assert!(!upsert_history(&mut entries, "a".to_string()));

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "a");
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

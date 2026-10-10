//! Comment-preserving, atomic, owner-only updates of TOML configuration files.
//!
//! [`TomlFile`] reads a file into a typed value and into a [`toml_edit`] document.
//! [`TomlFile::save`] compares the value the caller wants with the value that was
//! read and edits only the keys that differ, so comments, blank lines, key order
//! and keys the program doesn't know about survive. [`write_private`] replaces the
//! file atomically and keeps it readable only by its owner.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

/// Symbolic links [`write_private`] follows before giving up (Linux's `MAXSYMLINKS`).
const MAX_SYMLINK_HOPS: usize = 40;

/// Errors from reading or updating a TOML file through [`TomlFile`].
///
/// The message says what failed and names the file; the reason is the error's
/// [`source`](std::error::Error::source), so print the whole chain (anyhow's
/// `{:#}` or `{:?}`).
///
/// A TOML parse error quotes the line it failed on, so the chain can contain
/// text from the file. Show it to the file's owner or write it to a log, and
/// give anyone else only the message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TomlFileError {
    /// The file exists but could not be read.
    #[error("Failed to read {path}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why reading failed.
        source: io::Error,
    },
    /// The file is not valid TOML or doesn't match the expected structure. It is
    /// never written.
    #[error("Failed to parse {path}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// Where and why parsing failed.
        source: toml::de::Error,
    },
    /// The value can't be represented as TOML.
    #[error("Failed to serialize settings")]
    Serialize(#[from] toml::ser::Error),
    /// The new contents could not be written. The file is unchanged, except after
    /// an interrupted in-place rewrite (see [`write_private`]).
    #[error("Failed to write {path}")]
    Write {
        /// The file.
        path: PathBuf,
        /// Why writing failed. When [`TomlFile::save`] refused its own text
        /// because it did not read back as the intended values, this error's
        /// source is the parse error, or names the top-level keys that differ.
        source: io::Error,
    },
}

/// A TOML file read for a comment-preserving update.
///
/// ```no_run
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Clone, Default, Serialize, Deserialize)]
/// # #[serde(default)]
/// # struct Settings { theme: String }
/// use aranet_store::TomlFile;
///
/// let file = TomlFile::<Settings>::open("settings.toml")?;
/// let mut settings = file.value().clone();
/// settings.theme = "light".to_string();
/// // Only `theme` changes in the file; its comments and other keys stay as they are.
/// file.save(&settings)?;
/// # Ok::<(), aranet_store::TomlFileError>(())
/// ```
#[derive(Debug)]
pub struct TomlFile<T> {
    path: PathBuf,
    original: String,
    doc: DocumentMut,
    value: T,
}

impl<T: Serialize + DeserializeOwned> TomlFile<T> {
    /// Read and parse `path`.
    ///
    /// A missing file reads as an empty one, so every field takes its serde default.
    /// A file that doesn't parse, or doesn't match `T`, is an error, so it can never
    /// be overwritten through a `TomlFile`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TomlFileError> {
        let path = path.as_ref().to_path_buf();
        let original = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(TomlFileError::Read { path, source }),
        };
        let value = match toml::from_str(&original) {
            Ok(value) => value,
            Err(source) => return Err(TomlFileError::Parse { path, source }),
        };
        // The layout rules in `save` look for `\n`: edit a CRLF file as LF, and
        // `save` writes it back with CRLF.
        let doc = match original.replace("\r\n", "\n").parse::<DocumentMut>() {
            Ok(doc) => doc,
            Err(e) => {
                let source = <toml::de::Error as serde::de::Error>::custom(e);
                return Err(TomlFileError::Parse { path, source });
            }
        };
        Ok(Self {
            path,
            original,
            doc,
            value,
        })
    }

    /// The file this was read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The value the file held when it was opened.
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Write `desired`, changing only the keys whose values differ from
    /// [`value`](Self::value).
    ///
    /// Comments, blank lines, key order and keys that `T` doesn't know about are
    /// kept, and settings left at their defaults are not added. Returns `Ok(false)`
    /// without touching the file when nothing changed. The new text must read back
    /// as `desired` before it is written, atomically and owner-only, with
    /// [`write_private`]. If it doesn't, the file is left alone and the
    /// [`TomlFileError::Write`] that comes back says why in its source chain.
    pub fn save(mut self, desired: &T) -> Result<bool, TomlFileError> {
        let before = toml::Table::try_from(&self.value)?;
        let after = toml::Table::try_from(desired)?;
        if before == after {
            return Ok(false);
        }
        let root = self.doc.as_table();
        let old_keys: Vec<String> = root.iter().map(|(key, _)| key.to_string()).collect();
        let old_prefixes: Vec<(String, String)> = root
            .iter()
            .filter_map(|(key, item)| Some((key.to_string(), comment_prefix(item)?)))
            .collect();
        apply_table(self.doc.as_table_mut(), &before, &after);
        keep_comments_of_removed_tables(&mut self.doc, &old_prefixes);
        keep_trailing_comments_with_last_table(&mut self.doc, &old_keys);
        let mut rendered = self.doc.to_string();
        // Keep a CRLF file CRLF, unless that would change a multi-line string.
        if self.original.contains("\r\n") {
            let crlf = rendered.replace('\n', "\r\n");
            if read_back_failure::<T>(&crlf, &after)?.is_none() {
                rendered = crlf;
            }
        }
        if rendered == self.original {
            return Ok(false);
        }
        if let Some(reason) = read_back_failure::<T>(&rendered, &after)? {
            return Err(TomlFileError::Write {
                path: self.path,
                source: io::Error::other(NotReadBack(reason)),
            });
        }
        if let Err(source) = write_private(&self.path, rendered.as_bytes()) {
            return Err(TomlFileError::Write {
                path: self.path,
                source,
            });
        }
        Ok(true)
    }
}

/// The [`io::Error`] inside the [`TomlFileError::Write`] that [`TomlFile::save`]
/// returns when it refuses to write the text it rendered. Its source says why.
#[derive(Debug, thiserror::Error)]
#[error("rendered TOML does not read back as the intended values; the file was not changed")]
struct NotReadBack(#[source] ReadBackFailure);

/// How rendered text fails to read back as the values it was rendered from.
#[derive(Debug, thiserror::Error)]
enum ReadBackFailure {
    /// It is not valid TOML, or doesn't match the type.
    #[error(transparent)]
    Parse(toml::de::Error),
    /// It parses, but these top-level keys hold other values than intended. Only
    /// the names are given: a value may be a password or a token.
    #[error("these top-level keys read back with other values: {0:?}")]
    Differs(Vec<String>),
}

/// How `text`, parsed as `T` and serialised again, differs from `expected`, or
/// `None` when it gives exactly `expected`.
fn read_back_failure<T: Serialize + DeserializeOwned>(
    text: &str,
    expected: &toml::Table,
) -> Result<Option<ReadBackFailure>, TomlFileError> {
    let value = match toml::from_str::<T>(text) {
        Ok(value) => value,
        Err(e) => return Ok(Some(ReadBackFailure::Parse(e))),
    };
    let actual = toml::Table::try_from(&value)?;
    if actual == *expected {
        return Ok(None);
    }
    let unexpected = actual.keys().filter(|key| !expected.contains_key(*key));
    let differing = expected
        .keys()
        .chain(unexpected)
        .filter(|key| expected.get(*key) != actual.get(*key))
        .cloned()
        .collect();
    Ok(Some(ReadBackFailure::Differs(differing)))
}

/// The comment lines above a top-level `[table]`, or above the first entry of a
/// `[[table]]` array, including the blank lines around them.
fn comment_prefix(item: &Item) -> Option<String> {
    let table = match item {
        Item::Table(table) => table,
        Item::ArrayOfTables(tables) => tables.iter().next()?,
        _ => return None,
    };
    let prefix = table.decor().prefix()?.as_str()?;
    prefix.contains('#').then(|| prefix.to_owned())
}

/// A top-level table (or `[[array]]`) that disappeared keeps the comments above
/// it: they move to the end of the document, which is where
/// [`keep_trailing_comments_with_last_table`] took them from when the table was
/// added.
fn keep_comments_of_removed_tables(doc: &mut DocumentMut, old_prefixes: &[(String, String)]) {
    let mut kept = String::new();
    for (key, prefix) in old_prefixes {
        if !doc.as_table().contains_key(key) {
            kept.push_str(prefix.trim_end_matches('\n'));
            kept.push('\n');
        }
    }
    if kept.is_empty() {
        return;
    }
    let trailing = doc.trailing().as_str().unwrap_or("").to_owned();
    doc.set_trailing(format!("{kept}{trailing}"));
}

/// Comments after the last table's keys are the document's trailing trivia, and
/// `toml_edit` renders a table added by [`apply_table`] before them, which would
/// move them under the new table. Put them in front of the first added table
/// that has a header instead, so they stay with the table they follow.
fn keep_trailing_comments_with_last_table(doc: &mut DocumentMut, old_keys: &[String]) {
    let trailing = doc.trailing().as_str().unwrap_or("").to_owned();
    if trailing.trim().is_empty() {
        return;
    }
    let Some(table) = doc
        .as_table_mut()
        .iter_mut()
        .filter(|(key, _)| !old_keys.iter().any(|old| old == key.get()))
        .find_map(|(_, item)| first_rendered_table(item))
    else {
        return;
    };
    table.decor_mut().set_prefix(format!("{trailing}\n"));
    doc.set_trailing("");
}

/// The first table in `item` that `toml_edit` writes with a header, and so with
/// its decor: a `[[name]]` entry, or a `[name]` table that is explicit or has keys
/// of its own. An implicit table without keys (`[s]` of a new `[s.t]`) is never
/// written, so its subtables are searched instead.
fn first_rendered_table(item: &mut Item) -> Option<&mut Table> {
    match item {
        Item::ArrayOfTables(tables) => tables.iter_mut().next(),
        Item::Table(table) => {
            if table.is_implicit() && table.get_values().is_empty() {
                table
                    .iter_mut()
                    .find_map(|(_, child)| first_rendered_table(child))
            } else {
                Some(table)
            }
        }
        _ => None,
    }
}

/// Edit `target` so it holds `after`, touching only the keys whose values differ
/// from `before`.
fn apply_table(target: &mut dyn TableLike, before: &toml::Table, after: &toml::Table) {
    let empty = toml::Table::new();
    for (key, new) in after {
        let old = before.get(key);
        if old == Some(new) {
            continue;
        }
        let old_table = match old {
            Some(toml::Value::Table(table)) => table,
            _ => &empty,
        };
        let Some(item) = target.get_mut(key) else {
            insert_new(target, key, old_table, new);
            continue;
        };
        // A table (standard, inline or dotted): edit it key by key.
        if let toml::Value::Table(new_table) = new
            && let Some(table) = item.as_table_like_mut()
        {
            apply_table(table, old_table, new_table);
            if let Some(inline) = item.as_inline_table_mut() {
                // Inline tables are one line without comments: tidy the spacing.
                inline.fmt();
            }
            continue;
        }
        // `[[name]]` entries: keep unchanged entries and edit changed ones in place.
        if let (toml::Value::Array(new_items), Item::ArrayOfTables(tables)) = (new, &mut *item)
            && let Some(new_tables) = tables_in(new_items)
        {
            if new_tables.is_empty() {
                target.remove(key);
                continue;
            }
            let old_tables = match old {
                Some(toml::Value::Array(old_items)) => tables_in(old_items).unwrap_or_default(),
                _ => Vec::new(),
            };
            merge_tables(tables, &old_tables, &new_tables);
            continue;
        }
        // `name = []`, which 0.2 wrote for an empty device list, becomes `[[name]]`
        // sections again once the list has tables in it.
        if item.as_array().is_some_and(|array| array.is_empty())
            && let replacement @ Item::ArrayOfTables(_) = to_item(new)
        {
            *item = replacement;
            move_key_comments_to_first_table(target, key);
            continue;
        }
        match item {
            // Keep the value's surrounding whitespace and trailing comment.
            Item::Value(value) => {
                let decor = value.decor().clone();
                *value = to_value(new);
                *value.decor_mut() = decor;
            }
            other => *other = to_item(new),
        }
    }
    for key in before.keys() {
        if !after.contains_key(key) {
            target.remove(key);
        }
    }
}

/// `key = […]` became `[[key]]` sections: clear the key's decor, which was laid
/// out for `key = `, and put the comments above it above the first section.
fn move_key_comments_to_first_table(target: &mut dyn TableLike, key: &str) {
    let Some(mut name) = target.key_mut(key) else {
        return;
    };
    let comments = name
        .leaf_decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .filter(|prefix| prefix.contains('#'))
        .map(str::to_owned);
    name.leaf_decor_mut().clear();
    if let Some(comments) = comments
        && let Some(Item::ArrayOfTables(tables)) = target.get_mut(key)
        && let Some(first) = tables.get_mut(0)
    {
        first.decor_mut().set_prefix(comments);
    }
}

/// Add `key`, which `target` doesn't have. A table gets only the entries that
/// differ from `before` (usually the serde defaults), so a nested struct left at
/// its defaults never appears in the file.
fn insert_new(target: &mut dyn TableLike, key: &str, before: &toml::Table, new: &toml::Value) {
    let toml::Value::Table(new_table) = new else {
        target.insert(key, to_item(new));
        return;
    };
    let mut table = Table::new();
    table.set_implicit(true);
    table.set_dotted(target.is_dotted());
    target.insert(key, Item::Table(table));
    let Some(inserted) = target.get_mut(key).and_then(Item::as_table_like_mut) else {
        return;
    };
    apply_table(inserted, before, new_table);
    if inserted.is_empty() {
        target.remove(key);
    }
}

/// Update the entries of an array of tables. `tables` holds the file's entries in
/// the order of `before`. An entry equal to a wanted one is kept with its comments;
/// an entry whose first key has the same value as a wanted one (for `[[devices]]`,
/// the address) is edited key by key; other entries are removed, and wanted
/// entries without a match are written fresh. Comments that belong above the
/// array stay above whichever entry is now first.
fn merge_tables(tables: &mut ArrayOfTables, before: &[&toml::Table], after: &[&toml::Table]) {
    let header = take_header(tables);
    let mut sources: Vec<Option<usize>> = vec![None; after.len()];
    if tables.len() == before.len() {
        let mut used = vec![false; before.len()];
        for (source, wanted) in sources.iter_mut().zip(after) {
            if let Some(i) = (0..before.len()).find(|&i| !used[i] && before[i] == *wanted) {
                used[i] = true;
                *source = Some(i);
            }
        }
        for (source, wanted) in sources.iter_mut().zip(after) {
            if source.is_none()
                && let Some(i) =
                    (0..before.len()).find(|&i| !used[i] && same_first_entry(before[i], wanted))
            {
                used[i] = true;
                *source = Some(i);
            }
        }
    }
    // toml_edit writes parsed tables in their original order, so entries can only
    // be reused if they keep their relative order.
    if !sources.iter().flatten().is_sorted() {
        sources.fill(None);
    }
    let mut old: Vec<Option<Table>> = std::mem::take(tables).into_iter().map(Some).collect();
    for (source, wanted) in sources.into_iter().zip(after) {
        match source.and_then(|i| Some((i, old.get_mut(i)?.take()?))) {
            Some((i, mut table)) => {
                apply_table(&mut table, before[i], wanted);
                tables.push(table);
            }
            None => tables.push(to_table(wanted)),
        }
    }
    if let Some(header) = header
        && let Some(first) = tables.get_mut(0)
    {
        let own = first
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("");
        let prefix = format!("{header}{}", own.trim_start_matches('\n'));
        first.decor_mut().set_prefix(prefix);
    }
}

/// Take from the first entry's prefix the comments that a blank line separates
/// from the entry. They end the table above the array (that is where
/// [`keep_trailing_comments_with_last_table`] puts them) or open the file; they
/// are not the entry's own. The entry keeps a blank line and its own comments.
fn take_header(tables: &mut ArrayOfTables) -> Option<String> {
    let first = tables.get_mut(0)?;
    let prefix = first.decor().prefix()?.as_str()?;
    let (header, own) = prefix.split_at(prefix.rfind("\n\n")? + 2);
    if !header.contains('#') {
        return None;
    }
    let (header, own) = (header.to_owned(), format!("\n{own}"));
    first.decor_mut().set_prefix(own);
    Some(header)
}

/// The elements of `items` as tables, if every element is one.
fn tables_in(items: &[toml::Value]) -> Option<Vec<&toml::Table>> {
    items.iter().map(toml::Value::as_table).collect()
}

/// Whether two tables start with the same key and value.
fn same_first_entry(a: &toml::Table, b: &toml::Table) -> bool {
    matches!((a.iter().next(), b.iter().next()), (Some(x), Some(y)) if x == y)
}

/// A new document item for `value`: tables become `[name]` sections and arrays of
/// tables `[[name]]` sections.
fn to_item(value: &toml::Value) -> Item {
    match value {
        toml::Value::Table(table) => Item::Table(to_table(table)),
        toml::Value::Array(items) if !items.is_empty() => match tables_in(items) {
            Some(tables) => Item::ArrayOfTables(tables.into_iter().map(to_table).collect()),
            None => Item::Value(to_value(value)),
        },
        _ => Item::Value(to_value(value)),
    }
}

fn to_table(table: &toml::Table) -> Table {
    let mut out = Table::new();
    for (key, value) in table {
        out.insert(key, to_item(value));
    }
    out
}

/// A new inline value for `value`; tables inside arrays become inline tables.
fn to_value(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::from(s.as_str()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::from(*b),
        // toml 0.9 and toml_edit 0.25 use different toml_datetime versions, so
        // the datetime crosses over as text.
        toml::Value::Datetime(d) => d
            .to_string()
            .parse::<Value>()
            .expect("a TOML datetime re-parses"),
        toml::Value::Array(items) => Value::Array(items.iter().map(to_value).collect()),
        toml::Value::Table(table) => Value::InlineTable(
            table
                .iter()
                .map(|(key, value)| (key.as_str(), to_value(value)))
                .collect(),
        ),
    }
}

/// Replace `path` with `contents` atomically, readable only by its owner.
///
/// The contents go to a temporary file in the same directory, which is flushed to
/// disk and renamed over `path`, so a crash leaves either the old file or the new
/// one. A symbolic link at `path` is followed and stays a link.
///
/// On Unix a new file is created with mode 0600 and missing directories with
/// 0700; an existing file is replaced, so it ends up 0600 too. The replacement
/// keeps the old file's owner and group when the process may set them (as root,
/// so `sudo` doesn't take over a user's file). On Windows no permissions are
/// set: the file gets the access rules its directory passes on to new files.
///
/// Two kinds of existing file can't be replaced by a rename and are rewritten in
/// place instead, with a warning: a file that is itself a mount point (a
/// single-file bind mount into a container) and a file in a directory where the
/// process can't create files. An interrupted in-place rewrite can leave the file
/// truncated.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = resolve_symlinks(path)?;
    let dir = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} does not name a file", target.display()),
        )
    })?;
    create_private_dir(&dir)?;
    let exists = target.exists();

    let prefix = format!(".{}.", file_name.to_string_lossy());
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix).suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o600));
    }
    let mut temp = match builder.tempfile_in(&dir) {
        Ok(temp) => temp,
        Err(e)
            if exists
                && matches!(
                    e.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                ) =>
        {
            return rewrite_in_place(&target, contents, &e);
        }
        Err(e) => return Err(e),
    };
    // Keep the owner of the file being replaced. Only root can give a file to
    // another user; for anyone else this changes nothing or fails, which is fine.
    #[cfg(unix)]
    if let Ok(metadata) = fs::metadata(&target) {
        use std::os::unix::fs::{MetadataExt, fchown};
        let _ = fchown(temp.as_file(), Some(metadata.uid()), Some(metadata.gid()));
    }
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    if let Err(e) = temp.persist(&target) {
        if exists
            && matches!(
                e.error.kind(),
                io::ErrorKind::ResourceBusy | io::ErrorKind::CrossesDevices
            )
        {
            // Dropping the temporary file deletes it.
            drop(e.file);
            return rewrite_in_place(&target, contents, &e.error);
        }
        return Err(e.error);
    }
    // Make the rename itself durable. Best effort: not every file system can.
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(&dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Follow symbolic links from `path` to the file they finally name, which need
/// not exist yet.
fn resolve_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_SYMLINK_HOPS {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = fs::read_link(&current)?;
                current = match current.parent() {
                    Some(parent) if link.is_relative() => parent.join(link),
                    _ => link,
                };
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => return Ok(current),
        }
    }
    Err(io::Error::other(format!(
        "{}: too many levels of symbolic links",
        path.display()
    )))
}

/// Create `dir` and any missing parents, owner-only on Unix.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Overwrite `target` without replacing it: the fallback for files a rename can't
/// replace.
fn rewrite_in_place(target: &Path, contents: &[u8], cause: &io::Error) -> io::Result<()> {
    tracing::warn!(
        "{} could not be replaced atomically ({cause}); rewriting it in place",
        target.display()
    );
    let mut file = OpenOptions::new().write(true).open(target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                "Could not make {} readable only by its owner: {e}",
                target.display()
            );
        }
    }
    file.set_len(0)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Settings {
        name: Option<String>,
        x: i64,
        gui: Gui,
        labels: BTreeMap<String, String>,
        entries: Vec<Entry>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Gui {
        theme: String,
        width: i64,
    }

    impl Default for Gui {
        fn default() -> Self {
            Self {
                theme: "dark".to_string(),
                width: 800,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Entry {
        name: String,
        #[serde(default)]
        value: i64,
    }

    fn entry(name: &str, value: i64) -> Entry {
        Entry {
            name: name.to_string(),
            value,
        }
    }

    /// A file with comments, blank lines, a trailing comment and keys `Settings`
    /// doesn't know.
    const COMMENTED: &str = r#"# Settings for the tests.
x = 1
name = "office"   # shown in the title bar

unknown = "kept"

[gui]
# Colours
theme = "dark"
future_option = true

[[entries]]
name = "a"
value = 1
"#;

    /// The service's shipped Docker config: every table has comments, and the file
    /// ends in a block of commented-out settings. A Windows checkout may have
    /// converted it to CRLF (the repository has no `.gitattributes`), and the tests
    /// compare it with text written with LF. It is read at run time, not with
    /// `include_str!`: the published crate has no `docker/`, and its tests must
    /// still compile there. `None` outside a repository checkout.
    fn shipped_server_toml() -> Option<String> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !repo.join("crates/aranet-store/Cargo.toml").is_file() {
            return None;
        }
        let text = fs::read_to_string(repo.join("docker/server.toml"))
            .unwrap_or_else(|e| panic!("docker/server.toml: {e}"));
        Some(text.replace("\r\n", "\n"))
    }

    fn write_settings(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("settings.toml");
        fs::write(&path, text).unwrap();
        path
    }

    /// Open `path`, apply `change`, save, and return what the file then holds.
    fn update(path: &Path, change: impl FnOnce(&mut Settings)) -> String {
        let file = TomlFile::<Settings>::open(path).unwrap();
        let mut desired = file.value().clone();
        change(&mut desired);
        file.save(&desired).unwrap();
        fs::read_to_string(path).unwrap()
    }

    /// Everything in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn open_missing_file_reads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.toml");
        let file = TomlFile::<Settings>::open(&path).unwrap();
        assert_eq!(file.value(), &Settings::default());
        assert_eq!(file.path(), path);
    }

    #[test]
    fn save_changes_only_the_changed_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), COMMENTED);
        let saved = update(&path, |s| s.x = 2);
        assert_eq!(saved, COMMENTED.replace("x = 1", "x = 2"));
    }

    #[test]
    fn save_does_not_materialise_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let saved = update(&path, |s| s.gui.theme = "light".to_string());
        assert_eq!(saved, "[gui]\ntheme = \"light\"\n");
    }

    #[test]
    fn save_removes_key_set_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), COMMENTED);
        let saved = update(&path, |s| s.name = None);
        assert_eq!(
            saved,
            COMMENTED.replace("name = \"office\"   # shown in the title bar\n", "")
        );
    }

    #[test]
    fn save_keeps_trailing_comment_on_changed_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), "x = 1 # note\n");
        let saved = update(&path, |s| s.x = 2);
        assert_eq!(saved, "x = 2 # note\n");
    }

    #[test]
    fn save_into_inline_and_dotted_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(
            dir.path(),
            "x = 1\ngui.theme = \"dark\"\nlabels = { living = \"Living room\" }\n",
        );
        let saved = update(&path, |s| {
            s.gui.width = 1024;
            s.labels
                .insert("bedroom".to_string(), "Bedroom".to_string());
        });
        assert_eq!(
            saved,
            "x = 1\ngui.theme = \"dark\"\ngui.width = 1024\n\
             labels = { living = \"Living room\", bedroom = \"Bedroom\" }\n"
        );
    }

    #[test]
    fn untouched_toml_1_1_inline_table_is_kept_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        // A trailing comma inside an inline table is TOML 1.1, which toml 0.9 reads.
        let path = write_settings(dir.path(), "labels = { a = \"1\", }\nx = 1\n");
        let saved = update(&path, |s| s.x = 2);
        assert_eq!(saved, "labels = { a = \"1\", }\nx = 2\n");
    }

    #[test]
    fn save_is_noop_when_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), COMMENTED);
        let before = fs::metadata(&path).unwrap();

        let file = TomlFile::<Settings>::open(&path).unwrap();
        let unchanged = file.value().clone();
        assert!(!file.save(&unchanged).unwrap());

        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(after.ino(), before.ino());
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), COMMENTED);
    }

    #[test]
    fn open_refuses_unparseable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), "x = [1,\n");
        let error = TomlFile::<Settings>::open(&path).unwrap_err();
        assert!(matches!(error, TomlFileError::Parse { .. }), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "x = [1,\n");
    }

    /// The messages of `error` and of each error below it, outermost first.
    fn error_chain(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
        std::iter::successors(Some(error), |e| e.source())
            .map(ToString::to_string)
            .collect()
    }

    const NOT_READ_BACK: &str =
        "rendered TOML does not read back as the intended values; the file was not changed";

    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct Aliased {
        #[serde(alias = "colour")]
        color: String,
    }

    #[test]
    fn save_says_why_the_rendered_text_does_not_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), "colour = \"red\"\n");
        let file = TomlFile::<Aliased>::open(&path).unwrap();
        // The file names the field by its alias, so the save adds the field's
        // own name next to it, and the rendered text has the field twice.
        let desired = Aliased {
            color: "blue".to_string(),
        };

        let error = file.save(&desired).unwrap_err();

        let chain = error_chain(&error);
        assert_eq!(chain[0], format!("Failed to write {}", path.display()));
        assert_eq!(chain[1], NOT_READ_BACK);
        assert!(
            chain
                .get(2)
                .is_some_and(|reason| reason.contains("duplicate field `color`")),
            "{chain:#?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "colour = \"red\"\n");
    }

    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct WriteOnly {
        kept: i64,
        #[serde(skip_deserializing)]
        lost: i64,
    }

    #[test]
    fn save_names_the_keys_that_read_back_differently() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), "kept = 1\n");
        let file = TomlFile::<WriteOnly>::open(&path).unwrap();
        // `lost` is written but never read, so it reads back as its default.
        let desired = WriteOnly { kept: 2, lost: 5 };

        let error = file.save(&desired).unwrap_err();

        let chain = error_chain(&error);
        assert_eq!(chain[0], format!("Failed to write {}", path.display()));
        assert_eq!(chain[1], NOT_READ_BACK);
        assert_eq!(
            chain.get(2).map(String::as_str),
            Some(r#"these top-level keys read back with other values: ["lost"]"#),
            "{chain:#?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "kept = 1\n");
    }

    #[test]
    fn array_of_tables_keeps_comments_on_untouched_and_updated_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(
            dir.path(),
            r#"# first
[[entries]]
name = "a"
value = 1

# second
[[entries]]
name = "b"
value = 2

# third
[[entries]]
name = "c"
value = 3 # tuned
"#,
        );
        let saved = update(&path, |s| {
            s.entries = vec![entry("a", 1), entry("c", 30), entry("d", 4)];
        });
        assert_eq!(
            saved,
            r#"# first
[[entries]]
name = "a"
value = 1

# third
[[entries]]
name = "c"
value = 30 # tuned

[[entries]]
name = "d"
value = 4
"#
        );
    }

    #[test]
    fn array_of_tables_reorder_is_written_in_new_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(
            dir.path(),
            "[[entries]]\nname = \"a\"\nvalue = 1\n\n[[entries]]\nname = \"b\"\nvalue = 2\n",
        );
        let saved = update(&path, |s| s.entries.reverse());
        assert!(
            saved.find("\"b\"").unwrap() < saved.find("\"a\"").unwrap(),
            "{saved}"
        );
        let reread = TomlFile::<Settings>::open(&path).unwrap();
        assert_eq!(reread.value().entries, [entry("b", 2), entry("a", 1)]);
    }

    #[test]
    fn new_table_is_standard_table_after_existing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let original = "x = 1\n\n[[entries]]\nname = \"a\"\nvalue = 1\n";
        let path = write_settings(dir.path(), original);
        let saved = update(&path, |s| s.gui.theme = "light".to_string());
        assert_eq!(saved, format!("{original}\n[gui]\ntheme = \"light\"\n"));
    }

    #[test]
    fn new_table_keeps_trailing_comments_with_previous_table() {
        let dir = tempfile::tempdir().unwrap();
        // `# y = 2` is a commented-out key of `[a]`, which `Settings` doesn't know.
        let path = write_settings(dir.path(), "[a]\nx = 1\n# y = 2\n");
        let saved = update(&path, |s| s.entries.push(entry("n", 1)));
        assert_eq!(
            saved,
            "[a]\nx = 1\n# y = 2\n\n[[entries]]\nname = \"n\"\nvalue = 1\n"
        );
    }

    #[test]
    fn new_nested_table_keeps_trailing_comments_with_previous_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), "[a]\nx = 1\n# c1\n# c2\n");
        // `[s]` has no keys of its own, so only `[s.t]` gets a header.
        let file = TomlFile::<toml::Table>::open(&path).unwrap();
        let mut desired = file.value().clone();
        desired.extend(toml::from_str::<toml::Table>("s.t.v = 5").unwrap());
        file.save(&desired).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[a]\nx = 1\n# c1\n# c2\n\n[s.t]\nv = 5\n"
        );
    }

    #[test]
    fn removing_the_only_array_entry_restores_original_bytes() {
        let Some(shipped) = shipped_server_toml() else {
            return; // not a repository checkout: the published crate has no docker/
        };
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), &shipped);

        let added = update(&path, |s| s.entries.push(entry("n", 1)));
        assert!(
            added.ends_with("\n[[entries]]\nname = \"n\"\nvalue = 1\n"),
            "{added}"
        );
        let removed = update(&path, |s| s.entries.clear());

        assert_eq!(removed, shipped);
    }

    #[test]
    fn removing_the_first_array_entry_keeps_comments_of_the_table_above() {
        let Some(shipped) = shipped_server_toml() else {
            return; // not a repository checkout: the published crate has no docker/
        };
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(dir.path(), &shipped);
        update(&path, |s| s.entries.push(entry("a", 1)));
        update(&path, |s| s.entries.push(entry("b", 2)));

        // The commented-out `[influxdb]` settings are above the first entry now.
        let removed_first = update(&path, |s| {
            s.entries.remove(0);
        });
        assert_eq!(
            removed_first,
            format!("{shipped}\n[[entries]]\nname = \"b\"\nvalue = 2\n")
        );
        let removed_all = update(&path, |s| s.entries.clear());

        assert_eq!(removed_all, shipped);
    }

    #[test]
    fn new_first_array_entry_takes_comments_of_the_table_above() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(
            dir.path(),
            "[a]\nx = 1\n# y = 2\n\n# the first\n[[entries]]\nname = \"a\"\nvalue = 1\n",
        );
        let saved = update(&path, |s| s.entries.insert(0, entry("n", 1)));
        assert_eq!(
            saved,
            "[a]\nx = 1\n# y = 2\n\n[[entries]]\nname = \"n\"\nvalue = 1\n\n\
             # the first\n[[entries]]\nname = \"a\"\nvalue = 1\n"
        );
    }

    #[test]
    fn empty_inline_array_becomes_array_of_tables() {
        let dir = tempfile::tempdir().unwrap();
        // 0.2 saved an empty list as `name = []` at the top of the file.
        let path = write_settings(
            dir.path(),
            "# Sensors\nentries = []\n\n[gui]\ntheme = \"light\"\n",
        );
        let saved = update(&path, |s| s.entries = vec![entry("m", 1), entry("n", 2)]);
        assert_eq!(
            saved,
            "# Sensors\n[[entries]]\nname = \"m\"\nvalue = 1\n\n\
             [[entries]]\nname = \"n\"\nvalue = 2\n\n[gui]\ntheme = \"light\"\n"
        );
    }

    #[test]
    fn save_keeps_crlf_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_settings(
            dir.path(),
            "# Settings\r\nx = 1 # note\r\n\r\n[gui]\r\ntheme = \"dark\"\r\n",
        );
        let saved = update(&path, |s| {
            s.x = 2;
            s.entries.push(entry("n", 1));
        });
        assert_eq!(
            saved,
            "# Settings\r\nx = 2 # note\r\n\r\n[gui]\r\ntheme = \"dark\"\r\n\
             \r\n[[entries]]\r\nname = \"n\"\r\nvalue = 1\r\n"
        );
    }

    #[test]
    fn keys_needing_quotes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let labels: BTreeMap<String, String> = [
            ("living.room", r#"The "big" room"#),
            ("with space", r"C:\Users\aranet"),
            ("Küche", "Küche ☢"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

        let saved = update(&path, |s| s.labels = labels.clone());

        assert!(saved.contains("\"living.room\" = "), "{saved}");
        assert!(saved.contains("Küche"), "{saved}");
        assert_eq!(
            TomlFile::<Settings>::open(&path).unwrap().value().labels,
            labels
        );
    }

    #[test]
    fn write_private_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "old\n").unwrap();
        // A hard link keeps the old file's contents only if the file was replaced,
        // not truncated and rewritten.
        let link = dir.path().join("hard-link.toml");
        fs::hard_link(&path, &link).unwrap();

        write_private(&path, b"new\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&link).unwrap(), "old\n");
    }

    #[test]
    fn write_private_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        write_private(&path, b"x = 1\n").unwrap();
        write_private(&path, b"x = 2\n").unwrap();
        assert_eq!(names(dir.path()), ["settings.toml"]);

        // A directory can't be replaced by a file.
        let blocked = dir.path().join("blocked.toml");
        fs::create_dir(&blocked).unwrap();
        write_private(&blocked, b"x = 1\n").unwrap_err();
        assert_eq!(names(dir.path()), ["blocked.toml", "settings.toml"]);
    }

    #[cfg(unix)]
    mod unix {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, chown, symlink};

        use super::*;

        fn mode(path: &Path) -> u32 {
            fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        fn set_mode(path: &Path, mode: u32) {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        }

        fn inode(path: &Path) -> u64 {
            fs::metadata(path).unwrap().ino()
        }

        #[test]
        fn write_private_creates_0600_file_and_0700_dirs() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("a").join("b").join("settings.toml");
            write_private(&path, b"x = 1\n").unwrap();
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir.path().join("a")), 0o700);
            assert_eq!(mode(&dir.path().join("a").join("b")), 0o700);
        }

        #[test]
        fn write_private_tightens_existing_0644_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            fs::write(&path, "x = 1\n").unwrap();
            set_mode(&path, 0o644);
            let old_inode = inode(&path);

            write_private(&path, b"x = 2\n").unwrap();

            assert_eq!(mode(&path), 0o600);
            assert_ne!(
                inode(&path),
                old_inode,
                "the file should have been replaced"
            );
        }

        #[test]
        fn write_private_keeps_owner_of_replaced_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            fs::write(&path, "x = 1\n").unwrap();
            // Only root can give a file to another user (the aranet-ci image runs
            // as root).
            if fs::metadata(&path).unwrap().uid() != 0 {
                eprintln!("skipped: needs root to create a file owned by another user");
                return;
            }
            chown(&path, Some(4242), Some(4242)).unwrap();

            write_private(&path, b"x = 2\n").unwrap();

            let metadata = fs::metadata(&path).unwrap();
            assert_eq!((metadata.uid(), metadata.gid()), (4242, 4242));
            assert_eq!(mode(&path), 0o600);
            assert_eq!(fs::read_to_string(&path).unwrap(), "x = 2\n");
        }

        #[test]
        fn write_private_follows_symlink() {
            let dir = tempfile::tempdir().unwrap();
            let real = dir.path().join("real.toml");
            fs::write(&real, "x = 1\n").unwrap();
            let link = dir.path().join("link.toml");
            symlink("real.toml", &link).unwrap();

            write_private(&link, b"x = 2\n").unwrap();

            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read_to_string(&real).unwrap(), "x = 2\n");
            assert_eq!(names(dir.path()), ["link.toml", "real.toml"]);
        }

        #[test]
        fn write_private_falls_back_when_dir_not_writable() {
            let dir = tempfile::tempdir().unwrap();
            let locked = dir.path().join("locked");
            fs::create_dir(&locked).unwrap();
            let path = locked.join("settings.toml");
            fs::write(&path, "x = 1\n").unwrap();
            set_mode(&path, 0o644);
            let old_inode = inode(&path);
            set_mode(&locked, 0o500);

            // root ignores directory permissions (the aranet-ci image runs as root).
            let probe = locked.join("probe");
            if fs::File::create(&probe).is_ok() {
                fs::remove_file(&probe).unwrap();
                set_mode(&locked, 0o700);
                eprintln!("skipped: this user can create files in a 0500 directory");
                return;
            }

            let result = write_private(&path, b"x = 2\n");
            set_mode(&locked, 0o700); // let the TempDir clean up
            result.unwrap();

            assert_eq!(fs::read_to_string(&path).unwrap(), "x = 2\n");
            assert_eq!(
                inode(&path),
                old_inode,
                "the file should be rewritten in place"
            );
            assert_eq!(mode(&path), 0o600);
            assert_eq!(names(&locked), ["settings.toml"]);
        }

        /// Manual check: a single-file bind mount can't be renamed over (EBUSY),
        /// so the file is rewritten in place. The implementation plan (Phase 3b
        /// Task 7) has the `docker run` command.
        #[test]
        #[ignore = "needs ARANET_BIND_MOUNT_TEST naming a single-file bind mount"]
        fn bind_mount() {
            let path = PathBuf::from(
                std::env::var_os("ARANET_BIND_MOUNT_TEST")
                    .expect("set ARANET_BIND_MOUNT_TEST to a bind-mounted file"),
            );
            let old_inode = inode(&path);

            write_private(&path, b"a = 2\n").unwrap();

            assert_eq!(fs::read_to_string(&path).unwrap(), "a = 2\n");
            assert_eq!(
                inode(&path),
                old_inode,
                "the file should be rewritten in place"
            );
        }
    }
}

//! Per-binding diff: what's on the device vs what the manifest expects.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::bridges::Bridge;
use crate::config::{Binding, Device, Item, ItemKind};
use crate::manifest::{FileEntry, Manifest};

/// One file's worth of change between the device state and the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    /// Present on the device, missing from the manifest.
    New {
        path: PathBuf,
        size: u64,
        mtime: i64,
    },
    /// Present in both, but metadata (size or mtime) differs.
    Modified {
        path: PathBuf,
        size: u64,
        mtime: i64,
    },
    /// In the manifest, missing from the device.
    Deleted { path: PathBuf },
}

impl FileChange {
    pub fn path(&self) -> &Path {
        match self {
            Self::New { path, .. } | Self::Modified { path, .. } | Self::Deleted { path } => path,
        }
    }
}

/// All changes for a single `(item, device)` pairing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    pub item: String,
    pub device: String,
    pub changes: Vec<FileChange>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Compute the diff for one binding by listing the device through
/// `bridge`, applying `item`'s include/exclude filters, and comparing
/// each file's size + mtime against `manifest`.
///
/// Hash verification is intentionally **not** done here in v0.0.1: a
/// mismatching `(size, mtime)` is treated as a real modification. Hashes
/// are still recorded by the transfer step (Phase 3.3) so the manifest
/// stays correct; we just don't re-hash live files defensively.
pub fn compute_diff(
    bridge: &dyn Bridge,
    binding: &Binding,
    item: &Item,
    device: &Device,
    manifest: &Manifest,
) -> Result<Diff> {
    let root = binding.resolved_path(device);
    let empty = HashMap::new();
    let manifest_entries = manifest
        .items
        .get(&item.name)
        .map(|i| &i.files)
        .unwrap_or(&empty);

    let changes = match item.kind {
        ItemKind::Folder => folder_changes(bridge, &root, item, binding, manifest_entries)?,
        ItemKind::File => file_changes(bridge, &root, manifest_entries)?,
    };

    Ok(Diff {
        item: item.name.clone(),
        device: device.name.clone(),
        changes,
    })
}

fn folder_changes(
    bridge: &dyn Bridge,
    root: &Path,
    item: &Item,
    binding: &Binding,
    manifest_entries: &HashMap<String, FileEntry>,
) -> Result<Vec<FileChange>> {
    let live = bridge.list_files(root)?;
    let filters = Filters::compile(
        &item.include,
        &item.exclude,
        &binding.include,
        &binding.exclude,
    )
    .with_context(|| format!("compiling filters for item '{}'", item.name))?;
    let live: Vec<_> = live
        .into_iter()
        .filter(|f| filters.allow(&f.path))
        .collect();

    let live_keys: BTreeSet<String> = live
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();

    let mut changes = Vec::new();

    for f in &live {
        let key = f.path.to_string_lossy();
        match manifest_entries.get(key.as_ref()) {
            None => changes.push(FileChange::New {
                path: f.path.clone(),
                size: f.size,
                mtime: f.mtime,
            }),
            Some(entry) if entry.size != f.size || entry.mtime != f.mtime => {
                changes.push(FileChange::Modified {
                    path: f.path.clone(),
                    size: f.size,
                    mtime: f.mtime,
                });
            }
            Some(_) => {}
        }
    }

    for key in manifest_entries.keys() {
        if !live_keys.contains(key) {
            changes.push(FileChange::Deleted {
                path: PathBuf::from(key),
            });
        }
    }

    Ok(changes)
}

fn file_changes(
    bridge: &dyn Bridge,
    target: &Path,
    manifest_entries: &HashMap<String, FileEntry>,
) -> Result<Vec<FileChange>> {
    let key = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .with_context(|| format!("binding path has no file name: {}", target.display()))?;

    let live_meta = bridge.get_metadata(target).ok();
    let manifest_entry = manifest_entries.get(&key);

    Ok(match (live_meta, manifest_entry) {
        (Some(meta), None) => vec![FileChange::New {
            path: PathBuf::from(&key),
            size: meta.size,
            mtime: meta.mtime,
        }],
        (Some(meta), Some(entry)) if entry.size != meta.size || entry.mtime != meta.mtime => {
            vec![FileChange::Modified {
                path: PathBuf::from(&key),
                size: meta.size,
                mtime: meta.mtime,
            }]
        }
        (Some(_), Some(_)) => vec![],
        (None, Some(_)) => vec![FileChange::Deleted {
            path: PathBuf::from(&key),
        }],
        (None, None) => vec![],
    })
}

// ---------- glob filters ----------

/// Paths Redlight always refuses to sync, regardless of user's
/// include/exclude. Covers:
/// - Redlight's own bookkeeping (`.redlight/` on each device).
/// - macOS Finder/Spotlight/Trash detritus.
/// - Windows shell + recycle bin + system volume info.
/// - Linux trash + `lost+found`.
///
/// All patterns use `**/` prefix so they match at any depth (including
/// the binding root itself).
pub const SYSTEM_EXCLUDES: &[&str] = &[
    // Redlight itself.
    "**/.redlight/**",
    // macOS.
    "**/.DS_Store",
    "**/.Spotlight-V100/**",
    "**/.Trashes/**",
    "**/.fseventsd/**",
    "**/.TemporaryItems/**",
    "**/.AppleDouble/**",
    "**/.AppleDB/**",
    "**/.AppleDesktop/**",
    "**/._*",
    // Windows.
    "**/Thumbs.db",
    "**/ehthumbs.db",
    "**/desktop.ini",
    "**/$RECYCLE.BIN/**",
    "**/System Volume Information/**",
    // Linux / Unix-y.
    "**/lost+found/**",
    "**/.Trash-*/**",
];

struct Filters {
    /// Item-level allowlist. None = item doesn't restrict.
    item_include: Option<GlobSet>,
    /// Binding-level allowlist. None = binding doesn't narrow further.
    /// Combined by intersection with `item_include`.
    binding_include: Option<GlobSet>,
    /// Union of [`SYSTEM_EXCLUDES`] + item.exclude + binding.exclude.
    /// Any match blocks the file.
    exclude: GlobSet,
}

impl Filters {
    fn compile(
        item_include: &[String],
        item_exclude: &[String],
        binding_include: &[String],
        binding_exclude: &[String],
    ) -> Result<Self> {
        let item_include = if item_include.is_empty() {
            None
        } else {
            Some(build_set(item_include)?)
        };
        let binding_include = if binding_include.is_empty() {
            None
        } else {
            Some(build_set(binding_include)?)
        };
        let mut excludes: Vec<&str> = SYSTEM_EXCLUDES.to_vec();
        excludes.extend(item_exclude.iter().map(String::as_str));
        excludes.extend(binding_exclude.iter().map(String::as_str));
        let exclude = build_set_str(&excludes)?;
        Ok(Self {
            item_include,
            binding_include,
            exclude,
        })
    }

    fn allow(&self, path: &Path) -> bool {
        if let Some(inc) = &self.item_include
            && !inc.is_match(path)
        {
            return false;
        }
        if let Some(inc) = &self.binding_include
            && !inc.is_match(path)
        {
            return false;
        }
        !self.exclude.is_match(path)
    }
}

fn build_set_str(patterns: &[&str]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).with_context(|| format!("invalid glob: {p}"))?;
        b.add(g);
    }
    b.build().context("building glob set")
}

fn build_set(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).with_context(|| format!("invalid glob: {p}"))?;
        b.add(g);
    }
    b.build().context("building glob set")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs as stdfs;
    use std::time::UNIX_EPOCH;

    use tempfile::TempDir;

    use crate::bridges::FsBridge;
    use crate::config::{
        Binding, Bridge as BridgeKind, Device, DeviceMatch, DeviceType, Item, ItemKind, Role,
    };
    use crate::manifest::{FileEntry, Manifest};

    fn touch(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            stdfs::create_dir_all(parent).unwrap();
        }
        stdfs::write(&p, content).unwrap();
    }

    fn mtime_of(p: &Path) -> i64 {
        p.metadata()
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn make_item(name: &str, kind: ItemKind, include: &[&str], exclude: &[&str]) -> Item {
        Item {
            name: name.into(),
            kind,
            category: None,
            description: None,
            include: include.iter().map(|s| s.to_string()).collect(),
            exclude: exclude.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_drive(name: &str) -> Device {
        Device {
            name: name.into(),
            device_type: DeviceType::Drive,
            bridge: BridgeKind::Fs,
            matcher: DeviceMatch {
                volume_label: Some("LBL".into()),
                ..Default::default()
            },
            description: None,
        }
    }

    fn make_binding(item: &str, device: &str, path: Option<&str>) -> Binding {
        Binding {
            item: item.into(),
            device: device.into(),
            path: path.map(String::from),
            role: Role::ReadWrite,
            include: vec![],
            exclude: vec![],
        }
    }

    fn entry(size: u64, mtime: i64) -> FileEntry {
        FileEntry {
            hash: "x".into(),
            mtime,
            size,
            last_sync: mtime,
        }
    }

    // ---- Folder ----

    #[test]
    fn folder_new_file_detected() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "a.mp3", "data");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(
            matches!(diff.changes[0], FileChange::New { ref path, size: 4, .. } if path == &PathBuf::from("a.mp3"))
        );
    }

    #[test]
    fn folder_modified_when_size_differs() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "a.mp3", "newer");
        let mt = mtime_of(&tmp.path().join("a.mp3"));

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("music", "a.mp3", entry(3, mt)); // old size

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(
            matches!(diff.changes[0], FileChange::Modified { ref path, .. } if path == &PathBuf::from("a.mp3"))
        );
    }

    #[test]
    fn folder_modified_when_mtime_differs() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "a.mp3", "ab");
        let mt = mtime_of(&tmp.path().join("a.mp3"));

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("music", "a.mp3", entry(2, mt - 1000));

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(matches!(diff.changes[0], FileChange::Modified { .. }));
    }

    #[test]
    fn folder_unchanged_not_reported() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "a.mp3", "ab");
        let mt = mtime_of(&tmp.path().join("a.mp3"));

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("music", "a.mp3", entry(2, mt));

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert!(diff.is_empty());
    }

    #[test]
    fn folder_deleted_when_in_manifest_not_on_device() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        // Empty directory.

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("music", "gone.mp3", entry(10, 100));

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(
            matches!(diff.changes[0], FileChange::Deleted { ref path } if path == &PathBuf::from("gone.mp3"))
        );
    }

    #[test]
    fn folder_recursive_paths_are_relative_to_root() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "Beatles/Yesterday.mp3", "y");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert!(matches!(
            diff.changes[0],
            FileChange::New { ref path, .. } if path == &PathBuf::from("Beatles/Yesterday.mp3")
        ));
    }

    #[test]
    fn folder_include_filter_restricts() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "a.mp3", "a");
        touch(tmp.path(), "b.wav", "b");
        touch(tmp.path(), "Sub/c.mp3", "c");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &["**/*.mp3"], &[]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from([PathBuf::from("a.mp3"), PathBuf::from("Sub/c.mp3")])
        );
    }

    #[test]
    fn folder_exclude_filter_removes() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "song.mp3", "s");
        touch(tmp.path(), "draft-song.mp3", "d");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &["**/draft-*"]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("song.mp3")]));
    }

    #[test]
    fn folder_include_and_exclude_both_apply() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "ok.mp3", "1");
        touch(tmp.path(), "ok.wav", "2");
        touch(tmp.path(), "draft.mp3", "3");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &["**/*.mp3"], &["**/draft*"]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("ok.mp3")]));
    }

    // ---- File ----

    #[test]
    fn file_new_when_only_on_device() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "contrat.pdf", "abc");

        let device = make_drive("materia");
        let item = make_item("contrat", ItemKind::File, &[], &[]);
        let binding = make_binding("contrat", "materia", Some("/contrat.pdf"));
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(matches!(
            diff.changes[0],
            FileChange::New { ref path, size: 3, .. } if path == &PathBuf::from("contrat.pdf")
        ));
    }

    #[test]
    fn file_modified_when_size_differs() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "c.pdf", "longer");
        let mt = mtime_of(&tmp.path().join("c.pdf"));

        let device = make_drive("materia");
        let item = make_item("c", ItemKind::File, &[], &[]);
        let binding = make_binding("c", "materia", Some("/c.pdf"));
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("c", "c.pdf", entry(3, mt));

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(matches!(diff.changes[0], FileChange::Modified { .. }));
    }

    #[test]
    fn file_unchanged_not_reported() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "c.pdf", "ab");
        let mt = mtime_of(&tmp.path().join("c.pdf"));

        let device = make_drive("materia");
        let item = make_item("c", ItemKind::File, &[], &[]);
        let binding = make_binding("c", "materia", Some("/c.pdf"));
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("c", "c.pdf", entry(2, mt));

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert!(diff.is_empty());
    }

    #[test]
    fn file_deleted_when_only_in_manifest() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());

        let device = make_drive("materia");
        let item = make_item("c", ItemKind::File, &[], &[]);
        let binding = make_binding("c", "materia", Some("/c.pdf"));
        let mut manifest = Manifest::new("materia");
        manifest.update_entry("c", "c.pdf", entry(2, 100));

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(matches!(
            diff.changes[0],
            FileChange::Deleted { ref path } if path == &PathBuf::from("c.pdf")
        ));
    }

    // ---- glob compile errors ----

    // ---- binding-level filters ----

    fn binding_with_filters(
        item: &str,
        device: &str,
        include: &[&str],
        exclude: &[&str],
    ) -> Binding {
        Binding {
            item: item.into(),
            device: device.into(),
            path: None,
            role: Role::ReadWrite,
            include: include.iter().map(|s| s.to_string()).collect(),
            exclude: exclude.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn binding_include_narrows_item_include_by_intersection() {
        // Item allows all mp3 + flac; binding restricts to Beatles dir only.
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "Beatles/Help.mp3", "h");
        touch(tmp.path(), "Beatles/Yesterday.flac", "y");
        touch(tmp.path(), "Daft Punk/One More Time.mp3", "o");
        touch(tmp.path(), "Classical/Bach.mp3", "b");

        let device = make_drive("jarvis-like");
        let item = make_item("music", ItemKind::Folder, &["**/*.mp3", "**/*.flac"], &[]);
        let binding = binding_with_filters("music", "jarvis-like", &["**/Beatles/**"], &[]);
        let manifest = Manifest::new("jarvis-like");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from([
                PathBuf::from("Beatles/Help.mp3"),
                PathBuf::from("Beatles/Yesterday.flac"),
            ])
        );
    }

    #[test]
    fn binding_exclude_adds_to_item_exclude() {
        // Item excludes draft-*; binding additionally excludes Audiobooks.
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "song.mp3", "a");
        touch(tmp.path(), "draft-take.mp3", "b");
        touch(tmp.path(), "Audiobooks/book.mp3", "c");

        let device = make_drive("jarvis-like");
        let item = make_item("music", ItemKind::Folder, &[], &["**/draft-*"]);
        let binding = binding_with_filters("music", "jarvis-like", &[], &["**/Audiobooks/**"]);
        let manifest = Manifest::new("jarvis-like");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("song.mp3")]));
    }

    #[test]
    fn binding_include_and_exclude_combine() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "Beatles/Help.mp3", "1");
        touch(tmp.path(), "Beatles/draft-Help.mp3", "2"); // matches binding include but item-excluded
        touch(tmp.path(), "Beatles/Audiobook.mp3", "3"); // matches binding include but binding-excluded
        touch(tmp.path(), "Daft Punk/One.mp3", "4"); // not in binding include

        let device = make_drive("jarvis-like");
        let item = make_item("music", ItemKind::Folder, &["**/*.mp3"], &["**/draft-*"]);
        let binding = binding_with_filters(
            "music",
            "jarvis-like",
            &["**/Beatles/**"],
            &["**/Audiobook*"],
        );
        let manifest = Manifest::new("jarvis-like");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("Beatles/Help.mp3")]));
    }

    #[test]
    fn empty_binding_filters_preserve_item_filters() {
        // Backwards compat: a binding with no include/exclude behaves as
        // if those fields didn't exist.
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "song.mp3", "a");
        touch(tmp.path(), "song.wav", "b");

        let device = make_drive("any");
        let item = make_item("music", ItemKind::Folder, &["**/*.mp3"], &[]);
        let binding = binding_with_filters("music", "any", &[], &[]);
        let manifest = Manifest::new("any");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("song.mp3")]));
    }

    #[test]
    fn binding_include_alone_works_when_item_has_no_include() {
        // Item is wide open (no include); binding alone restricts.
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), "ok.mp3", "a");
        touch(tmp.path(), "Special/keeper.txt", "b");
        touch(tmp.path(), "random.txt", "c");

        let device = make_drive("any");
        let item = make_item("anything", ItemKind::Folder, &[], &[]);
        let binding = binding_with_filters("anything", "any", &["**/Special/**"], &[]);
        let manifest = Manifest::new("any");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("Special/keeper.txt")]));
    }

    // ---- system excludes ----

    #[test]
    fn folder_skips_redlight_bookkeeping() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), ".redlight/manifest.toml", "x");
        touch(tmp.path(), ".redlight/sync_log.toml", "y");
        touch(tmp.path(), "song.mp3", "s");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("song.mp3")]));
    }

    #[test]
    fn folder_skips_macos_detritus() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), ".DS_Store", "x");
        touch(tmp.path(), "Sub/.DS_Store", "x");
        touch(tmp.path(), "real.mp3", "r");

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &[]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("real.mp3")]));
    }

    #[test]
    fn user_excludes_still_apply_alongside_system() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        touch(tmp.path(), ".redlight/x", "1"); // skipped by system
        touch(tmp.path(), "draft-a.mp3", "2"); // skipped by user
        touch(tmp.path(), "real.mp3", "3"); // kept

        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &[], &["**/draft-*"]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let diff = compute_diff(&bridge, &binding, &item, &device, &manifest).unwrap();
        let paths: BTreeSet<_> = diff
            .changes
            .iter()
            .map(|c| c.path().to_path_buf())
            .collect();
        assert_eq!(paths, BTreeSet::from([PathBuf::from("real.mp3")]));
    }

    #[test]
    fn invalid_glob_returns_error() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());
        let device = make_drive("materia");
        let item = make_item("music", ItemKind::Folder, &["[bad"], &[]);
        let binding = make_binding("music", "materia", None);
        let manifest = Manifest::new("materia");

        let err = compute_diff(&bridge, &binding, &item, &device, &manifest)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid glob") || err.contains("filters"));
    }
}

//! Native filesystem events are bounded refresh hints; actual Git/source reads remain authoritative.
use crate::worker::LatestWorker;
use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_BURST: Duration = Duration::from_secs(1);
const DIRECTORY_LIMIT: usize = 8192;
const DIRECTORY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct Signal {
    pending: Mutex<Option<(Instant, Instant)>>,
    failed: AtomicBool,
    topology: AtomicBool,
}
impl Signal {
    /// Coalesces arbitrarily many notifications into first/last timestamps, retaining no paths or diagnostics.
    fn mark(&self, now: Instant) {
        let mut pending = self.pending.lock().expect("watch signal");
        match pending.as_mut() {
            Some((_, last)) => *last = now,
            None => *pending = Some((now, now)),
        }
    }
    /// Bounds continuous event storms as well as quiet bursts; callers consume only when able to queue reconciliation.
    fn take_due(&self, now: Instant) -> bool {
        let mut pending = self.pending.lock().expect("watch signal");
        if pending.is_some_and(|(first, last)| {
            now.saturating_duration_since(last) >= DEBOUNCE
                || now.saturating_duration_since(first) >= MAX_BURST
        }) {
            *pending = None;
            true
        } else {
            false
        }
    }
    /// Ignores reads/siblings; replaced roots invalidate native health, while errors/overflow retain bounded actual reconciliation hints.
    fn event(&self, event: notify::Result<Event>, roots: &[PathBuf]) {
        let event = match event {
            Ok(event) => event,
            Err(_) => {
                self.failed.store(true, Ordering::Release);
                self.mark(Instant::now());
                return;
            }
        };
        let access_only = matches!(event.kind, EventKind::Access(_))
            && !matches!(
                event.kind,
                EventKind::Access(AccessKind::Close(AccessMode::Write | AccessMode::Any))
            );
        if matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        ) && event.paths.iter().any(|path| roots.contains(path))
        {
            self.failed.store(true, Ordering::Release);
        }
        let relevant = event.paths.is_empty()
            || event
                .paths
                .iter()
                .any(|path| roots.iter().any(|root| path.starts_with(root)));
        if event.need_rescan()
            || relevant
                && matches!(
                    event.kind,
                    EventKind::Create(CreateKind::Folder)
                        | EventKind::Remove(RemoveKind::Folder)
                        | EventKind::Modify(ModifyKind::Name(_))
                )
        {
            self.topology.store(true, Ordering::Release);
        }
        if event.need_rescan() || !access_only && relevant {
            self.mark(Instant::now());
        }
    }
}
struct LiveWatch {
    _watcher: RecommendedWatcher,
}
struct Ready {
    signal: Arc<Signal>,
    native: bool,
}

/// Enumerates one bounded directory set on the worker; nonrecursive registrations cannot expand it inside notify.
fn bounded_directories(roots: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let mut paths = roots.to_vec();
    let mut bytes = paths.iter().map(PathBuf::capacity).sum::<usize>();
    let mut next = 0;
    while next < paths.len() {
        let path = &paths[next];
        next += 1;
        let Ok(entries) = std::fs::read_dir(path) else {
            return None;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return None;
            };
            let Ok(kind) = entry.file_type() else {
                return None;
            };
            if kind.is_dir() && !kind.is_symlink() {
                let path = entry.path();
                bytes += path.capacity();
                if paths.len() >= DIRECTORY_LIMIT || bytes > DIRECTORY_BYTES {
                    return None;
                }
                paths.push(path);
            }
        }
    }
    Some(paths)
}

/// Builds the native watcher only on its worker; all metadata locations come from core's actual gix repository context.
fn install(state: &mut Option<LiveWatch>, root: Option<PathBuf>) -> Option<Ready> {
    *state = None;
    let root = root?;
    let signal = Arc::new(Signal::default());
    let build = || -> Option<RecommendedWatcher> {
        let locations = hunkra_core::repository_locations(&root).ok()?;
        let mut roots = vec![
            locations.root.clone(),
            locations.git_dir,
            locations.common_dir,
        ];
        roots.sort_by_key(|path| path.components().count());
        let mut recursive = Vec::<PathBuf>::new();
        for path in &roots {
            if !recursive.iter().any(|root| path.starts_with(root)) {
                recursive.push(path.clone());
            }
        }
        let mut directories = bounded_directories(&recursive)?;
        let tree_count = directories.len();
        for path in &recursive {
            if let Some(parent) = path.parent()
                && !directories.iter().any(|path| path == parent)
            {
                directories.push(parent.to_path_buf());
            }
        }
        if directories.len() > DIRECTORY_LIMIT
            || directories.iter().map(PathBuf::capacity).sum::<usize>() > DIRECTORY_BYTES
        {
            return None;
        }
        directories.sort();
        let callback_signal = signal.clone();
        let callback_roots = roots.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event| callback_signal.event(event, &callback_roots),
            Config::default().with_follow_symlinks(false),
        )
        .ok()?;
        for path in &directories {
            watcher.watch(path, RecursiveMode::NonRecursive).ok()?;
        }
        // Close the census-to-registration gap: later structural changes are already watched, while earlier ones require another bounded setup.
        let verified = bounded_directories(&recursive)?;
        if verified.len() != tree_count
            || verified
                .iter()
                .any(|path| directories.binary_search(path).is_err())
        {
            signal.topology.store(true, Ordering::Release);
            signal.mark(Instant::now());
        }
        Some(watcher)
    };
    let watcher = build();
    let native = watcher.is_some();
    *state = watcher.map(|watcher| LiveWatch { _watcher: watcher });
    if !native {
        signal.failed.store(true, Ordering::Release);
    }
    Some(Ready { signal, native })
}

pub struct RepositoryWatch {
    worker: LatestWorker<Option<PathBuf>, Option<Ready>>,
    generation: u64,
    ready: Option<Ready>,
    requested: bool,
    root: Option<PathBuf>,
}
impl Default for RepositoryWatch {
    /// Creates one bounded setup worker; recursive discovery and watcher drop never block the UI thread.
    fn default() -> Self {
        Self {
            worker: LatestWorker::with_state("hunkra-watch", || None, install),
            generation: 0,
            ready: None,
            requested: false,
            root: None,
        }
    }
}
impl RepositoryWatch {
    /// Drops the old UI signal immediately and coalesces setup requests; obsolete replies/events cannot refresh another project.
    pub fn reset(&mut self, root: Option<PathBuf>) {
        self.generation += 1;
        self.ready = None;
        self.requested = root.is_some();
        self.root = root.clone();
        self.worker.submit(self.generation, root);
    }
    /// Applies only the latest setup result; native failures retain periodic reconciliation as an independent path.
    pub fn poll(&mut self) -> bool {
        if let Some((generation, ready)) = self.worker.take()
            && generation == self.generation
        {
            self.ready = ready;
            return true;
        }
        false
    }
    /// Reports native health without exposing paths, OS errors or arbitrary library messages.
    pub fn active(&self) -> bool {
        self.ready
            .as_ref()
            .is_some_and(|ready| ready.native && !ready.signal.failed.load(Ordering::Acquire))
    }
    /// Explains a completed unsupported/failed setup, while a pending setup does not display a premature failure.
    pub fn unavailable(&self) -> bool {
        self.requested && self.ready.is_some() && !self.active()
    }
    /// Consumes a bounded hint and coalesces directory recensus on the existing worker; replaced roots still require explicit Refresh.
    pub fn take_due(&mut self) -> bool {
        let due = self
            .ready
            .as_ref()
            .is_some_and(|ready| ready.signal.take_due(Instant::now()));
        if due
            && self.active()
            && self
                .ready
                .as_ref()
                .unwrap()
                .signal
                .topology
                .swap(false, Ordering::AcqRel)
        {
            self.reset(self.root.clone());
        }
        due
    }
    /// Preserves a periodic safety net for event loss/native limits without running a second library polling scanner.
    pub fn interval(&self) -> Duration {
        Duration::from_secs(if self.active() { 5 } else { 2 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
    use std::{fs, path::Path, process::Command};

    #[cfg(target_os = "linux")]
    /// Counts the actual inotify registrations belonging to this fixture inode, excluding concurrent tests and platform watchers.
    fn native_registrations(root: &Path) -> usize {
        use std::os::unix::fs::MetadataExt;
        let inode = format!("ino:{:x}", root.metadata().unwrap().ino());
        fs::read_dir("/proc/self/fdinfo")
            .unwrap()
            .filter_map(|entry| fs::read_to_string(entry.ok()?.path()).ok())
            .filter(|data| {
                data.lines()
                    .any(|line| line.split_whitespace().any(|part| part == inode))
            })
            .map(|data| {
                data.lines()
                    .filter(|line| line.starts_with("inotify wd:"))
                    .count()
            })
            .sum()
    }

    #[cfg(target_os = "linux")]
    #[test]
    /// New subtrees cannot make notify grow its private kernel watch map beyond the explicitly installed directory set.
    fn native_maps_do_not_expand_outside_the_directory_budget() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        let mut state = None;
        let ready = install(&mut state, Some(root.clone())).unwrap();
        assert!(ready.native);
        let before = native_registrations(&root);
        assert!(before > 0);
        for number in 0..128 {
            fs::create_dir_all(root.join(format!("new-{number}/a/b/c/d/e"))).unwrap();
        }
        wait(|| ready.signal.take_due(Instant::now()));
        let after = native_registrations(&root);
        assert!(
            after <= before,
            "native map expanded outside our budget: {before} -> {after}"
        );
    }

    #[test]
    /// New and renamed directory trees are re-enumerated on the existing worker before observing later child writes.
    fn changed_directory_topology_rearms_without_git_writes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        let head = fs::read(root.join(".git/HEAD")).unwrap();
        let mut watch = RepositoryWatch::default();
        watch.reset(Some(root.clone()));
        wait(|| watch.poll());
        assert!(watch.active());
        let nested = root.join("new/a/b");
        fs::create_dir_all(&nested).unwrap();
        wait(|| watch.take_due());
        wait(|| watch.poll());
        assert!(watch.active());
        fs::write(nested.join("f"), b"descendant\r\nEOF").unwrap();
        wait(|| watch.take_due());
        assert!(watch.active());
        fs::rename(root.join("new"), root.join("moved")).unwrap();
        wait(|| watch.take_due());
        wait(|| watch.poll());
        assert!(watch.active());
        fs::write(root.join("moved/a/b/f"), b"renamed descendant\n").unwrap();
        wait(|| watch.take_due());
        assert!(watch.active());
        assert_eq!(fs::read(root.join(".git/HEAD")).unwrap(), head);
        assert!(!root.join(".git/index").exists());
    }

    #[test]
    /// Exceeding the directory budget retires native registrations while actual reads and explicit recovery remain available.
    fn runtime_directory_budget_degrades_and_recovers() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        let mut watch = RepositoryWatch::default();
        watch.reset(Some(root.clone()));
        wait(|| watch.poll());
        assert!(watch.active());
        let budget = root.join("budget");
        fs::create_dir(&budget).unwrap();
        for number in 0..DIRECTORY_LIMIT {
            fs::create_dir(budget.join(number.to_string())).unwrap();
        }
        wait(|| watch.take_due());
        wait(|| watch.poll());
        assert!(watch.unavailable() && !watch.active());
        assert_eq!(watch.interval(), Duration::from_secs(2));
        assert!(hunkra_core::scan(&root).unwrap().files.is_empty());
        #[cfg(target_os = "linux")]
        wait(|| native_registrations(&root) == 0);
        fs::remove_dir_all(&budget).unwrap();
        watch.reset(Some(root.clone()));
        wait(|| watch.poll());
        assert!(watch.active() && !watch.unavailable());
        fs::write(root.join("f"), b"recovered\n").unwrap();
        wait(|| watch.take_due());
        assert_eq!(hunkra_core::scan(&root).unwrap().files.len(), 1);
    }

    /// Waits with a finite deadline for a real event/setup; tests never spin indefinitely.
    fn wait(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "native watch deadline");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// Runs Git only inside the fixture with isolated identity/config and no owner repository environment.
    fn git(root: &Path, args: &[&str]) {
        let result = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.name=Watch Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    #[test]
    /// Read opens and unrelated sibling events cannot self-refresh; continuous write storms still become one bounded due hint.
    fn access_storms_debounce_and_errors_are_bounded() {
        let signal = Signal::default();
        let roots = vec![PathBuf::from("/fixture")];
        for _ in 0..1000 {
            signal.event(
                Ok(
                    Event::new(EventKind::Access(AccessKind::Open(AccessMode::Read)))
                        .add_path("/fixture/f".into()),
                ),
                &roots,
            );
        }
        signal.event(
            Ok(Event::new(EventKind::Any).add_path("/sibling/f".into())),
            &roots,
        );
        assert!(!signal.take_due(Instant::now() + Duration::from_secs(2)));
        let now = Instant::now();
        for number in 0..1000 {
            signal.mark(now + Duration::from_millis(number));
        }
        assert!(!signal.take_due(now + Duration::from_millis(999)));
        assert!(signal.take_due(now + Duration::from_secs(1)));
        assert!(!signal.take_due(now + Duration::from_secs(3)));
        signal.event(
            Err(notify::Error::generic("private arbitrary diagnostic")),
            &roots,
        );
        assert!(
            signal.failed.load(Ordering::Acquire)
                && signal.take_due(Instant::now() + Duration::from_secs(2))
        );
    }
    #[test]
    /// Real atomically replaced files request actual Git status; quiet readonly scans do not create an access loop.
    fn native_replace_and_project_reset_are_observed() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("f"), b"one\n").unwrap();
        git(&root, &["add", "f"]);
        git(&root, &["commit", "-m", "base"]);
        let mut watch = RepositoryWatch::default();
        watch.reset(Some(root.clone()));
        wait(|| watch.poll());
        assert!(watch.active());
        let index = fs::read(root.join(".git/index")).unwrap();
        for _ in 0..3 {
            assert!(hunkra_core::scan(&root).unwrap().files.is_empty());
        }
        std::thread::sleep(Duration::from_millis(250));
        assert!(!watch.take_due());
        fs::write(root.join("next"), b"two\n").unwrap();
        fs::rename(root.join("next"), root.join("f")).unwrap();
        wait(|| watch.take_due());
        assert_eq!(hunkra_core::scan(&root).unwrap().files.len(), 1);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        watch.reset(None);
        wait(|| watch.poll());
        assert!(!watch.active() && !watch.unavailable());
        fs::write(root.join("f"), b"three\n").unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!watch.take_due());
    }
    #[test]
    /// Linked worktree common refs/config and worktree files trigger their own signal without watching symlink targets.
    fn linked_common_metadata_and_symlinks_are_separate() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("f"), b"base\n").unwrap();
        git(&root, &["add", "f"]);
        git(&root, &["commit", "-m", "base"]);
        let linked = temporary.path().join("linked");
        git(
            &root,
            &["worktree", "add", "-b", "topic", linked.to_str().unwrap()],
        );
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, linked.join("foreign")).unwrap();
        let mut watch = RepositoryWatch::default();
        watch.reset(Some(linked.clone()));
        wait(|| watch.poll());
        assert!(watch.active());
        let locations = hunkra_core::repository_locations(&linked).unwrap();
        assert_ne!(locations.git_dir, locations.common_dir);
        fs::write(
            locations.common_dir.join("refs/heads/observed"),
            fs::read(locations.common_dir.join("refs/heads/main")).unwrap(),
        )
        .unwrap();
        wait(|| watch.take_due());
        fs::write(linked.join("f"), b"changed\n").unwrap();
        wait(|| watch.take_due());
        fs::write(outside.join("private"), b"outside\n").unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!watch.take_due());
    }
    #[test]
    /// Unsupported setup preserves finite periodic reconciliation and never treats missing native events as clean Git state.
    fn unavailable_setup_is_explicit() {
        let temporary = tempfile::tempdir().unwrap();
        let mut watch = RepositoryWatch::default();
        watch.reset(Some(temporary.path().join("missing")));
        wait(|| watch.poll());
        assert!(watch.unavailable() && !watch.active());
        assert_eq!(watch.interval(), Duration::from_secs(2));
    }

    #[test]
    /// Structural changes of an exact watched root invalidate native health while ordinary descendant writes remain hints.
    fn replaced_roots_require_a_new_native_setup() {
        let roots = vec![
            PathBuf::from("/fixture/repo"),
            PathBuf::from("/fixture/common"),
        ];
        for kind in [
            EventKind::Create(CreateKind::Folder),
            EventKind::Remove(RemoveKind::Folder),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
        ] {
            let signal = Signal::default();
            signal.event(
                Ok(Event::new(kind).add_path(roots[0].join("inside"))),
                &roots,
            );
            assert!(!signal.failed.load(Ordering::Acquire));
            signal.event(Ok(Event::new(kind).add_path(roots[1].clone())), &roots);
            assert!(
                signal.failed.load(Ordering::Acquire),
                "replaced root still reported healthy: {kind:?}"
            );
            assert!(signal.take_due(Instant::now() + Duration::from_secs(2)));
        }
    }

    #[test]
    /// A real root directory swap preserves actual reconciliation and manual rearming observes new descendants without modifying index bytes.
    fn native_root_replacement_can_be_rearmed() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("f"), b"original\n").unwrap();
        git(&root, &["add", "f"]);
        git(&root, &["commit", "-m", "original"]);
        let mut watch = RepositoryWatch::default();
        watch.reset(Some(root.clone()));
        wait(|| watch.poll());
        assert!(watch.active());
        fs::rename(&root, temporary.path().join("displaced")).unwrap();
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("f"), b"replacement\n").unwrap();
        git(&root, &["add", "f"]);
        git(&root, &["commit", "-m", "replacement"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        wait(|| watch.take_due());
        assert!(
            watch.unavailable() && !watch.active(),
            "obsolete root watches stayed healthy"
        );
        assert_eq!(watch.interval(), Duration::from_secs(2));
        assert!(hunkra_core::scan(&root).unwrap().files.is_empty());
        watch.reset(Some(root.clone()));
        wait(|| watch.poll());
        assert!(watch.active() && !watch.unavailable());
        fs::write(root.join("f"), b"new descendant change\n").unwrap();
        wait(|| watch.take_due());
        assert_eq!(hunkra_core::scan(&root).unwrap().files.len(), 1);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    }
}

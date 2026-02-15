use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Event sent from the watcher to the main loop.
#[derive(Debug)]
pub struct FileChanged {
    pub session_id: String,
    pub path: PathBuf,
}

/// Watches JSONL files for changes and sends events to the main loop.
///
/// Bridges notify's sync callback to tokio's async world by using an
/// unbounded channel internally and a spawned tokio task to forward events.
pub struct SessionWatcher {
    watcher: RecommendedWatcher,
    session_to_path: HashMap<String, PathBuf>,
    path_to_session: Arc<Mutex<HashMap<PathBuf, String>>>,
}

impl SessionWatcher {
    /// Create a new watcher. Spawns a background tokio task that bridges
    /// notify's sync callback to the async `tx` channel.
    pub fn new(tx: mpsc::Sender<FileChanged>) -> notify::Result<Self> {
        let path_to_session: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Unbounded channel to bridge sync notify callback -> async tokio task.
        let (raw_tx, mut raw_rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();

        // Create the notify watcher with a sync callback that forwards raw paths.
        let watcher = RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    // Only forward data-modification events (file content changes).
                    let dominated = matches!(
                        event.kind,
                        notify::EventKind::Modify(ModifyKind::Data(_))
                            | notify::EventKind::Modify(ModifyKind::Any)
                            | notify::EventKind::Any
                    );
                    if dominated {
                        for path in event.paths {
                            let _ = raw_tx.send(path);
                        }
                    }
                }
            },
            Config::default(),
        )?;

        // Spawn a tokio task that reads raw path events, looks up the session
        // ID from the shared map, and forwards FileChanged events to the main
        // loop channel.
        let bridge_map = Arc::clone(&path_to_session);
        tokio::spawn(async move {
            while let Some(path) = raw_rx.recv().await {
                let session_id = {
                    let map = bridge_map.lock().expect("path_to_session lock poisoned");
                    map.get(&path).cloned()
                };
                if let Some(session_id) = session_id {
                    let event = FileChanged {
                        session_id,
                        path: path.clone(),
                    };
                    if tx.send(event).await.is_err() {
                        // Main loop receiver dropped; stop the bridge task.
                        break;
                    }
                }
            }
        });

        Ok(Self {
            watcher,
            session_to_path: HashMap::new(),
            path_to_session,
        })
    }

    /// Start watching a JSONL file for a session.
    ///
    /// If this session is already being watched, the old watch is removed
    /// first before the new one is installed.
    pub fn watch(&mut self, session_id: &str, path: &PathBuf) -> notify::Result<()> {
        // If already watching this session, remove old watch first.
        if self.session_to_path.contains_key(session_id) {
            self.unwatch(session_id);
        }

        // Register with the OS file watcher (non-recursive, single file).
        self.watcher.watch(path, RecursiveMode::NonRecursive)?;

        // Update both maps.
        self.session_to_path
            .insert(session_id.to_string(), path.clone());
        {
            let mut map = self
                .path_to_session
                .lock()
                .expect("path_to_session lock poisoned");
            map.insert(path.clone(), session_id.to_string());
        }

        Ok(())
    }

    /// Stop watching a session's JSONL file.
    pub fn unwatch(&mut self, session_id: &str) {
        if let Some(path) = self.session_to_path.remove(session_id) {
            // Best-effort: ignore errors from unwatch (e.g. file already gone).
            let _ = self.watcher.unwatch(&path);
            let mut map = self
                .path_to_session
                .lock()
                .expect("path_to_session lock poisoned");
            map.remove(&path);
        }
    }

    /// Check if a session is currently being watched.
    pub fn is_watching(&self, session_id: &str) -> bool {
        self.session_to_path.contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use std::time::Duration;
    use tokio::time::timeout;

    /// Create a unique temp directory for a test. Returns the path.
    /// The caller is responsible for cleanup.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agent-dash-test-watcher-{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("failed to create test dir");
        dir
    }

    #[tokio::test]
    async fn watcher_detects_file_change() {
        let dir = test_dir("detect");
        let file_path = dir.join("session.jsonl");

        // Create the file so notify can watch it.
        std::fs::write(&file_path, "").expect("failed to create file");

        let (tx, mut rx) = mpsc::channel::<FileChanged>(16);
        let mut watcher = SessionWatcher::new(tx).expect("failed to create watcher");

        watcher
            .watch("sess-1", &file_path)
            .expect("failed to watch");

        // Give the watcher a moment to register.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Append to the file to trigger a change event.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .expect("failed to open file");
            writeln!(f, r#"{{"type":"user","sessionId":"sess-1"}}"#)
                .expect("failed to write");
        }

        // Wait for the event (with timeout).
        let event = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for FileChanged event")
            .expect("channel closed unexpectedly");

        assert_eq!(event.session_id, "sess-1");
        assert_eq!(event.path, file_path);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unwatch_removes_watch() {
        let dir = test_dir("unwatch");
        let file_path = dir.join("session.jsonl");
        std::fs::write(&file_path, "").expect("failed to create file");

        let (tx, _rx) = mpsc::channel::<FileChanged>(16);
        let mut watcher = SessionWatcher::new(tx).expect("failed to create watcher");

        watcher
            .watch("sess-2", &file_path)
            .expect("failed to watch");
        assert!(watcher.is_watching("sess-2"));

        watcher.unwatch("sess-2");
        assert!(!watcher.is_watching("sess-2"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn watch_replaces_existing() {
        let dir = test_dir("replace");
        let file_a = dir.join("a.jsonl");
        let file_b = dir.join("b.jsonl");
        std::fs::write(&file_a, "").expect("failed to create file a");
        std::fs::write(&file_b, "").expect("failed to create file b");

        let (tx, _rx) = mpsc::channel::<FileChanged>(16);
        let mut watcher = SessionWatcher::new(tx).expect("failed to create watcher");

        watcher
            .watch("sess-3", &file_a)
            .expect("failed to watch a");
        assert!(watcher.is_watching("sess-3"));

        // Watching the same session with a different file should replace it.
        watcher
            .watch("sess-3", &file_b)
            .expect("failed to watch b");
        assert!(watcher.is_watching("sess-3"));

        // The internal path_to_session map should only have file_b.
        let map = watcher.path_to_session.lock().unwrap();
        assert!(!map.contains_key(&file_a));
        assert!(map.contains_key(&file_b));

        drop(map);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn is_watching_returns_false_for_unknown() {
        let (tx, _rx) = mpsc::channel::<FileChanged>(16);
        let watcher = SessionWatcher::new(tx).expect("failed to create watcher");
        assert!(!watcher.is_watching("nonexistent"));
    }
}

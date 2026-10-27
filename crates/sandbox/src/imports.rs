//! Demos dropped onto the window: each is imported into the cache on a thread of its own (crates/source-demo), then read
//! with the rest of the data. A demo already in the cache is not parsed again; a cs2-throws file dropped is read as it
//! is. The shell shows one line while it works and for a while after.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nades_source_file::FileSource;

/// How long the last result stays on screen.
const SHOWN: Duration = Duration::from_secs(20);

#[derive(Debug, Default)]
struct Progress {
    /// The demo being read and the share done.
    current: Option<(String, f32)>,
    /// Paths dropped and not yet started.
    queued: usize,
    /// What the last import did, and when.
    last: Option<(String, Instant)>,
    /// Files added to the source since the window last asked.
    added: usize,
}

pub struct Imports {
    progress: Arc<Mutex<Progress>>,
    send: Sender<PathBuf>,
}

impl Imports {
    /// A worker that imports into `cache` and adds what it imports to `files`; `wake` asks the window to look.
    pub fn start(files: Arc<FileSource>, cache: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (send, receive) = channel();
        let shared = progress.clone();
        std::thread::Builder::new().name("imports".into()).spawn(move || work(&receive, &files, &cache, &shared, wake.as_ref())).expect("an imports thread");
        Self { progress, send }
    }

    /// Imports what was dropped: a demo, a cs2-throws file, or a folder of either.
    pub fn dropped(&self, path: PathBuf) {
        self.progress.lock().expect("imports").queued += 1;
        let _ = self.send.send(path);
    }

    /// The line the shell shows, if any.
    pub fn line(&self) -> Option<String> {
        let p = self.progress.lock().expect("imports");
        let more = if p.queued > 0 { format!(" ({} more queued)", p.queued) } else { String::new() };
        if let Some((name, done)) = &p.current {
            return Some(format!("Reading {name}… {:.0}%{more}", done * 100.0));
        }
        p.last.as_ref().filter(|(_, at)| at.elapsed() < SHOWN).map(|(line, _)| line.clone())
    }

    /// Whether files were added since the last call: the window then reads its lists again.
    pub fn take_added(&self) -> bool {
        std::mem::take(&mut self.progress.lock().expect("imports").added) > 0
    }
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn work(receive: &Receiver<PathBuf>, files: &FileSource, cache: &Path, progress: &Mutex<Progress>, wake: &(dyn Fn() + Send + Sync)) {
    let set = |f: &dyn Fn(&mut Progress)| {
        f(&mut progress.lock().expect("imports"));
        wake();
    };
    while let Ok(path) = receive.recv() {
        set(&|p| p.queued = p.queued.saturating_sub(1));
        let result = import(&path, files, cache, &|current| set(&|p| p.current = current.clone()));
        let (line, new) = match result {
            Ok(done) => done,
            Err(e) => (format!("Could not read {}: {e:#}", name(&path)), 0),
        };
        eprintln!("data: {line}");
        set(&|p| {
            p.current = None;
            p.last = Some((line.clone(), Instant::now()));
            p.added += new;
        });
    }
}

/// One dropped path: its demos imported and every file added. Says what happened, and how many files are new to the
/// source.
fn import(path: &Path, files: &FileSource, cache: &Path, show: &dyn Fn(Option<(String, f32)>)) -> anyhow::Result<(String, usize)> {
    let demos = nades_source_demo::demos_in(path)?;
    let mut added = Vec::new();
    let mut throws = 0;
    for demo in &demos {
        let label = name(demo);
        let mut last = -1.0f32;
        let done = nades_source_demo::import(demo, cache, &mut |share| {
            if share - last >= 0.01 || share >= 1.0 {
                last = share;
                show(Some((label.clone(), share)));
            }
        })?;
        throws += done.throws;
        added.push(done.file);
    }
    if demos.is_empty() {
        // a cs2-throws file, or a folder of them
        if path.is_dir() {
            added.extend(nades_source_file::expand(std::slice::from_ref(&path.to_path_buf()))?);
        } else if nades_source_file::is_throws_file(path) {
            added.push(path.to_path_buf());
        }
    }
    if added.is_empty() {
        anyhow::bail!("it holds no demo and no cs2-throws file");
    }
    let mut new = 0;
    for file in &added {
        new += usize::from(files.add(file)?);
    }
    let line = match demos.len() {
        0 => format!("Added {}: {} file(s)", name(path), added.len()),
        1 => format!("Added {}: {throws} throws", name(path)),
        n => format!("Added {n} demos from {}: {throws} throws", name(path)),
    };
    Ok((line, new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nades_model::{MapName, ThrowSource};
    use nades_source_demo::extract::{Facts, ThrowFact};

    fn wait(imports: &Imports, until: impl Fn(&Imports) -> bool) {
        let started = Instant::now();
        while !until(imports) {
            assert!(started.elapsed() < Duration::from_secs(20), "{:?}", imports.line());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_dropped_file_joins_the_data_and_a_dropped_stranger_says_why_not() {
        let dir = std::env::temp_dir().join(format!("nades-sandbox-imports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let demo = dir.join("match.dem");
        std::fs::write(&demo, b"the facts below stand for it").unwrap();
        let throw = ThrowFact { entity: 7, tick: 640, last_tick: 700, nade: "smoke", side: Some("CT"), start: [0.0, 0.0, 64.0], velocity: [1.0, 0.0, 0.0], detonate_tick: Some(700), land: [100.0, 0.0, 0.0], ..Default::default() };
        let file = dir.join("throws.sqlite");
        nades_source_demo::write(&Facts { map: "de_nuke".into(), throws: vec![throw], ..Default::default() }, "ab", &demo, &file).unwrap();

        let files = Arc::new(FileSource::open(&[]).unwrap());
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = woken.clone();
        let imports = Imports::start(files.clone(), dir.join("cache"), Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
        imports.dropped(file.clone());
        wait(&imports, |i| i.line().is_some_and(|l| l.starts_with("Added")));
        assert_eq!(imports.line().as_deref(), Some("Added throws.sqlite: 1 file(s)"));
        assert!(imports.take_added() && !imports.take_added(), "told once");
        assert_eq!(files.map(&MapName("de_nuke".into())).unwrap().throws.len(), 1);
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed) > 0);

        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();
        imports.dropped(notes);
        wait(&imports, |i| i.line().is_some_and(|l| l.starts_with("Could not")));
        assert_eq!(imports.line().as_deref(), Some("Could not read notes.txt: it holds no demo and no cs2-throws file"));
        assert!(!imports.take_added());
    }
}

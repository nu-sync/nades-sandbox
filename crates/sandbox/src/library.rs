//! The lineups and their stages, read and built off the window's thread.
//!
//! - **The data thread** reads the database (`nades-data`): one map's lineup list with the teams and players its
//!   filters offer, then the scene and the viewers of each listed lineup's example throw, the selected one first, each
//!   batch in one brief read-only open. A file `nades` holds is waited out.
//! - **The collider** (awpy's mesh and its BVH) is built once per map on a thread of its own.
//! - **Stages** are built on every core (rayon) once the collider is there: the cloud and its light, the fire, the
//!   cameras, the result shot through the collider. The window thread only picks a built one up, so a lineup switch
//!   costs it a texture upload and a frame.
//!
//! Every reply wakes the window's event loop.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use glam::DVec3;
use nades_data::{DataError, Database, Filter, Lineup, Throw};
use nades_scene::camera::{ShotCamera, Viewer};
use nades_scene::collider::Collider;
use nades_scene::smoke::{smoke_volume, voxel_light};
use nades_scene::stage::{Stage, build_stage};

/// A throw ready to show: its stage, its result shot, and its smoke's lit volume.
pub struct Prepared {
    pub throw: Throw,
    pub viewers: usize,
    pub stage: Stage,
    pub result: ShotCamera,
    /// 32³ RGBA texels, lit against the collider.
    pub volume: Option<Vec<u8>>,
    /// How long the stage took to build (cloud, light, cameras), in ms.
    pub build_ms: f64,
    /// Of which the smoke's light.
    pub light_ms: f64,
}

impl Prepared {
    pub fn new(throw: Throw, viewers: &[nades_data::Viewer], collider: Option<&Collider>, toward_sun: DVec3) -> Self {
        let started = Instant::now();
        let stage = build_stage(&throw);
        let viewers: Vec<Viewer> = viewers.iter().map(Viewer::from).collect();
        let result = stage.result_camera(&viewers, collider);
        let lit = Instant::now();
        let volume = stage.cloud.as_ref().map(|cloud| {
            let light = collider.map(|c| voxel_light(cloud, toward_sun, c));
            smoke_volume(cloud, light.as_ref())
        });
        let light_ms = if stage.cloud.is_some() { lit.elapsed().as_secs_f64() * 1e3 } else { 0.0 };
        Self { throw, viewers: viewers.len(), stage, result, volume, build_ms: started.elapsed().as_secs_f64() * 1e3, light_ms }
    }
}

/// A collider being built: None until the build is done, then the collider (None when the map has no mesh).
type Slot = Mutex<Option<Option<Arc<Collider>>>>;

/// A map's collider, once built: what stage builds wait for.
#[derive(Clone, Default)]
pub struct SharedCollider(Arc<(Slot, Condvar)>);

impl SharedCollider {
    /// Builds `map`'s collider on its own thread from awpy's mesh in `maps`.
    pub fn build(maps: PathBuf, map: String, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Self::default();
        let filled = shared.clone();
        std::thread::Builder::new()
            .name("collider".into())
            .spawn(move || {
                let started = Instant::now();
                let collider = Collider::load(&maps.join(format!("{map}.mesh")));
                match &collider {
                    Ok(c) => eprintln!("collider: {map}, {} triangles in {:.1} ms", c.triangles(), started.elapsed().as_secs_f64() * 1e3),
                    Err(e) => eprintln!("collider: {e} (result shots without line of sight, the smoke unlit)"),
                }
                let (slot, ready) = &*filled.0;
                *slot.lock().expect("collider slot") = Some(collider.ok().map(Arc::new));
                ready.notify_all();
                wake();
            })
            .expect("a collider thread");
        shared
    }

    /// The collider, waiting for the build (at most ten seconds); None when the map has no mesh.
    fn wait(&self, shutdown: &Shutdown) -> Option<Arc<Collider>> {
        let (slot, ready) = &*self.0;
        let mut built = slot.lock().expect("collider slot");
        let started = Instant::now();
        while built.is_none() && started.elapsed() < Duration::from_secs(10) && !shutdown.requested() {
            (built, _) = ready.wait_timeout(built, Duration::from_millis(100)).expect("collider slot");
        }
        built.clone().flatten()
    }

}

pub enum Request {
    /// A map's lineups for a filter, with the teams and players the filters offer.
    List(Filter),
    /// Stages for these throws (the first first), on `map`, lit with `toward_sun`, against `collider`.
    Prepare { map: String, throws: Vec<String>, collider: SharedCollider, toward_sun: DVec3 },
    Shutdown,
}

pub enum Reply {
    List { filter: Filter, lineups: Vec<Lineup>, teams: Vec<String>, players: Vec<String>, ms: f64 },
    Prepared { map: String, prepared: Arc<Prepared> },
    Failed(String),
}

/// The data thread's handle: requests go in, replies come out (each with a wake-up).
pub struct Library {
    requests: Sender<Work>,
    pub replies: Receiver<Reply>,
    pub database: Database,
    worker: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<Shutdown>,
}

struct Work {
    request: Request,
    priority: Priority,
}

/// Whom a request is for, in the order the data thread serves them: the shown map (its list, a lineup just picked and
/// its neighbours), then the maps kept warm beside it (their lists and selected stages), then the rest of the shown
/// map's stages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Priority {
    Shown,
    Warm,
    Bulk,
}

#[derive(Default)]
struct Shutdown {
    requested: Mutex<bool>,
    changed: Condvar,
}

impl Shutdown {
    fn request(&self) {
        *self.requested.lock().expect("library shutdown") = true;
        self.changed.notify_all();
    }

    fn requested(&self) -> bool {
        *self.requested.lock().expect("library shutdown")
    }

    fn wait(&self, duration: Duration) -> bool {
        let requested = self.requested.lock().expect("library shutdown");
        let (requested, _) = self.changed.wait_timeout_while(requested, duration, |requested| !*requested).expect("library shutdown");
        *requested
    }
}

/// How long a read waits for `nades` to let go of the file before saying so.
const PATIENCE: Duration = Duration::from_secs(600);

impl Library {
    pub fn start(database: Database, wake: impl Fn() + Send + Sync + Clone + 'static) -> Self {
        let (requests, inbox) = std::sync::mpsc::channel::<Work>();
        let (outbox, replies) = std::sync::mpsc::channel::<Reply>();
        let db = database.clone();
        let shutdown = Arc::new(Shutdown::default());
        let stopping = shutdown.clone();
        let worker = std::thread::Builder::new()
            .name("data".into())
            .spawn(move || serve(db, inbox, outbox, wake, &stopping))
            .expect("a data thread");
        Self { requests, replies, database, worker: Some(worker), shutdown }
    }

    pub fn send(&self, request: Request) {
        let _ = self.requests.send(Work { request, priority: Priority::Shown });
    }

    /// For a map kept warm beside the shown one: after the shown map's own requests.
    pub fn send_background(&self, request: Request) {
        let _ = self.requests.send(Work { request, priority: Priority::Warm });
    }

    /// The shown map's stages beyond the first few: after what keeps the other maps warm.
    pub fn send_bulk(&self, request: Request) {
        let _ = self.requests.send(Work { request, priority: Priority::Bulk });
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        self.shutdown.request();
        let _ = self.requests.send(Work { request: Request::Shutdown, priority: Priority::Shown });
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!("the data worker failed during shutdown");
        }
    }
}

/// The data thread: lists answered as they come, newest wins; stage reads batched, the newest request first.
fn serve(db: Database, inbox: Receiver<Work>, outbox: Sender<Reply>, wake: impl Fn() + Send + Sync + Clone + 'static, shutdown: &Shutdown) {
    let mut queue: VecDeque<Work> = VecDeque::new();
    loop {
        if shutdown.requested() { return; }
        if queue.is_empty() {
            match inbox.recv() {
                Ok(r) => queue.push_back(r),
                Err(_) => return,
            }
        }
        while let Ok(r) = inbox.try_recv() {
            queue.push_back(r);
        }
        if shutdown.requested() { return; }
        let work = next(&mut queue);
        let priority = work.priority;
        match work.request {
            Request::List(filter) => {
                let started = Instant::now();
                let read = patiently(&db, &outbox, &wake, shutdown, |c| {
                    let lineups = nades_data::lineups(c, &filter)?;
                    let teams = nades_data::teams(c, &filter.map)?;
                    let players = nades_data::players(c, &filter.map, filter.team.as_deref())?;
                    Ok((lineups, teams, players))
                });
                if shutdown.requested() { return; }
                let reply = match read {
                    Ok((lineups, teams, players)) => Reply::List { filter, lineups, teams, players, ms: started.elapsed().as_secs_f64() * 1e3 },
                    Err(e) => Reply::Failed(failure(&db, e)),
                };
                let _ = outbox.send(reply);
                wake();
            }
            Request::Prepare { map, throws, collider, toward_sun } => {
                // in slices, so a newer request (a lineup just picked) is not kept waiting behind a long list
                let mut rest = throws;
                while !rest.is_empty() {
                    if shutdown.requested() { return; }
                    let slice: Vec<String> = rest.drain(..rest.len().min(8)).collect();
                    let read = patiently(&db, &outbox, &wake, shutdown, |c| {
                        let mut rows = Vec::new();
                        for id in &slice {
                            if let Some(throw) = nades_data::throw_scene(c, id)? {
                                let viewers = nades_data::scene_viewers(c, id)?;
                                rows.push((throw, viewers));
                            }
                        }
                        Ok(rows)
                    });
                    if shutdown.requested() { return; }
                    match read {
                        Ok(rows) => {
                            // the stages need the collider; build them on every core, each reply waking the window
                            let collider = collider.wait(shutdown);
                            if shutdown.requested() { return; }
                            for (throw, viewers) in rows {
                                let (outbox, wake, collider, map) = (outbox.clone(), wake.clone(), collider.clone(), map.clone());
                                rayon::spawn(move || {
                                    let prepared = Arc::new(Prepared::new(throw, &viewers, collider.as_deref(), toward_sun));
                                    let _ = outbox.send(Reply::Prepared { map, prepared });
                                    wake();
                                });
                            }
                        }
                        Err(e) => {
                            let _ = outbox.send(Reply::Failed(failure(&db, e)));
                            wake();
                            break;
                        }
                    }
                    // something newer came in: it goes first, and the rest of this list after it
                    if let Ok(r) = inbox.try_recv() {
                        queue.push_back(Work { request: Request::Prepare { map: map.clone(), throws: std::mem::take(&mut rest), collider: collider.clone(), toward_sun }, priority });
                        queue.push_back(r);
                        break;
                    }
                }
            }
            Request::Shutdown => return,
        }
    }
}

/// The request to serve next: the shown map's newest list (its older ones no longer matter), else its newest request
/// (the lineup just picked); else the oldest warm-up of a kept map; else the oldest of the rest.
fn next(queue: &mut VecDeque<Work>) -> Work {
    let shown_list = |w: &Work| w.priority == Priority::Shown && matches!(w.request, Request::List(_));
    if let Some(at) = queue.iter().rposition(shown_list) {
        let list = queue.remove(at).expect("present");
        queue.retain(|w| !shown_list(w));
        return list;
    }
    let at = queue.iter().rposition(|w| w.priority == Priority::Shown).or_else(|| queue.iter().position(|w| w.priority == Priority::Warm)).unwrap_or(0);
    queue.remove(at).expect("not empty")
}

/// A read that says so, once, when `nades` holds the file, then waits for it.
fn patiently<T>(db: &Database, outbox: &Sender<Reply>, wake: &impl Fn(), shutdown: &Shutdown, mut read: impl FnMut(&nades_data::duckdb::Connection) -> nades_data::duckdb::Result<T>) -> Result<T, DataError> {
    let started = Instant::now();
    let mut announced = false;
    loop {
        if shutdown.requested() { return Err(DataError::Busy); }
        match db.read(&mut read) {
            Err(DataError::Busy) => {
                if !announced {
                    let _ = outbox.send(Reply::Failed("nades is writing the database (an ingest?): the lineups follow when it is done".into()));
                    wake();
                    announced = true;
                }
                if started.elapsed() >= PATIENCE || shutdown.wait(Duration::from_millis(500)) { return Err(DataError::Busy); }
            }
            other => return other,
        }
    }
}

fn failure(db: &Database, e: DataError) -> String {
    match e {
        DataError::Missing(path) => format!("no database at {} (set NADES_DB)", path.display()),
        other => format!("{} ({})", other, db.path().display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_library_waits_for_its_native_database_worker() {
        let (requests, _inbox) = std::sync::mpsc::channel();
        let (_outbox, replies) = std::sync::mpsc::channel();
        let (started, active) = std::sync::mpsc::channel();
        let (release, proceed) = std::sync::mpsc::channel();
        let (finished, native_done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            {
                let connection = nades_data::duckdb::Connection::open_in_memory().unwrap();
                started.send(()).unwrap();
                proceed.recv().unwrap();
                let actual: i64 = connection.query_row("select 42", [], |row| row.get(0)).unwrap();
                assert_eq!(actual, 42);
            }
            finished.send(()).unwrap();
        });
        let library = Library {
            requests, replies, database: Database::stored(":memory:"), worker: Some(worker), shutdown: Arc::new(Shutdown::default()),
        };
        active.recv_timeout(Duration::from_secs(5)).unwrap();
        let (dropped, complete) = std::sync::mpsc::channel();
        let dropping = std::thread::spawn(move || { drop(library); dropped.send(()).unwrap(); });
        assert!(matches!(complete.recv_timeout(Duration::from_millis(50)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
        release.send(()).unwrap();
        complete.recv_timeout(Duration::from_secs(5)).unwrap();
        native_done.recv_timeout(Duration::from_secs(1)).unwrap();
        dropping.join().unwrap();
    }

    #[test]
    fn shutdown_wakes_a_busy_database_retry_without_waiting_for_patience() {
        let shutdown = Arc::new(Shutdown::default());
        let waiting = shutdown.clone();
        let (started, active) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            waiting.wait(PATIENCE)
        });
        active.recv_timeout(Duration::from_secs(1)).unwrap();
        shutdown.request();
        assert!(worker.join().unwrap());
    }

    #[test]
    fn shutdown_does_not_wait_for_an_unbuilt_collider() {
        let shutdown = Shutdown::default();
        shutdown.request();
        assert!(SharedCollider::default().wait(&shutdown).is_none());
    }

    #[test]
    fn the_shown_map_goes_first_then_the_warm_maps_then_the_rest_of_its_stages() {
        let list = |map: &str, priority| Work { request: Request::List(Filter::map(map)), priority };
        let stages = |map: &str, throw: &str, priority| Work { request: Request::Prepare { map: map.into(), throws: vec![throw.into()], collider: SharedCollider::default(), toward_sun: DVec3::Z }, priority };
        let mut queue: VecDeque<Work> = [
            stages("de_cache", "bulk", Priority::Bulk),
            list("de_mirage", Priority::Warm),
            list("de_cache", Priority::Shown),
            stages("de_nuke", "nuke first", Priority::Warm),
            stages("de_cache", "picked", Priority::Shown),
            list("de_cache", Priority::Shown),
        ]
        .into();
        let served: Vec<String> = std::iter::from_fn(|| (!queue.is_empty()).then(|| next(&mut queue)))
            .map(|w| match w.request {
                Request::List(f) => format!("list {}", f.map),
                Request::Prepare { throws, .. } => throws[0].clone(),
                Request::Shutdown => "shutdown".into(),
            })
            .collect();
        // the shown map's older list is dropped
        assert_eq!(served, ["list de_cache", "picked", "list de_mirage", "nuke first", "bulk"]);
    }
}

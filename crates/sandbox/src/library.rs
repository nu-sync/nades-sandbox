//! The lineups and their stages, read and built off the window's thread.
//!
//! - **The data thread** reads the source (a `ThrowSource`: the cs2-throws files, through nades-source-file) and keeps
//!   one `nades_model::Library` per map, built once per revision of the data: the map's facts with the sandbox's rules
//!   applied. A list is that library cut by the filters, in memory, with the teams and players they offer. A stage
//!   reads the throw's tracks and the samples of its viewpoint window, the selected lineup's first. A producer writing
//!   the data is waited out.
//! - **The collider** (awpy's mesh and its BVH) is built once per map on a thread of its own.
//! - **Stages** are built on every core (rayon) once the collider is there: the cloud and its light, the fire, the
//!   cameras, the result shot through the collider. The window thread only picks a built one up, so a lineup switch
//!   costs it a texture upload and a frame.
//!
//! Every reply wakes the window's event loop.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use glam::DVec3;
use nades_model::{Demo, Derived, Effect, Library as MapLibrary, Lineup, LineupQuery, MapName, Revision, SourceError, Throw, ThrowId, ThrowSource, Tracks, Viewpoint};
use nades_scene::camera::{ShotCamera, Viewer};
use nades_scene::collider::Collider;
use nades_scene::smoke::{smoke_volume, voxel_light};
use nades_scene::stage::{Stage, build_stage};

/// A throw ready to show: its facts and derivations, its stage, its result shot, and its smoke's lit volume.
pub struct Prepared {
    pub throw: Throw,
    pub derived: Derived,
    /// Its demo, for the shell (the file, the date, the producer's tags).
    pub demo: Option<Demo>,
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
    pub fn new(throw: Throw, derived: Derived, demo: Option<Demo>, tracks: &Tracks, viewpoints: &[Viewpoint], collider: Option<&Collider>, toward_sun: DVec3) -> Self {
        let started = Instant::now();
        let stage = build_stage(&throw, tracks, &derived);
        let viewers: Vec<Viewer> = viewpoints.iter().map(Viewer::from).collect();
        let result = stage.result_camera(&viewers, collider);
        let lit = Instant::now();
        let volume = stage.cloud.as_ref().map(|cloud| {
            let light = collider.map(|c| voxel_light(cloud, toward_sun, c));
            smoke_volume(cloud, light.as_ref())
        });
        let light_ms = if stage.cloud.is_some() { lit.elapsed().as_secs_f64() * 1e3 } else { 0.0 };
        Self { throw, derived, demo, viewers: viewers.len(), stage, result, volume, build_ms: started.elapsed().as_secs_f64() * 1e3, light_ms }
    }

    /// `id`'s stage on `library`'s map, read from `source`; None when the source has no such throw.
    pub fn read(source: &dyn ThrowSource, library: &MapLibrary, id: &ThrowId, collider: Option<&Collider>, toward_sun: DVec3) -> Result<Option<Self>, SourceError> {
        Ok(read_stage(source, library, id)?.map(|s| Self::new(s.throw, s.derived, s.demo, &s.tracks, &s.viewpoints, collider, toward_sun)))
    }
}

/// What a stage is built from: a throw's facts and derivations, its demo, its tracks and its viewpoints.
pub struct StageFacts {
    pub throw: Throw,
    pub derived: Derived,
    pub demo: Option<Demo>,
    pub tracks: Tracks,
    pub viewpoints: Vec<Viewpoint>,
}

/// Reads what `id`'s stage needs: its tracks (a flash's blinded players placed from the samples when the facts lack
/// their positions), and the samples of its viewpoint window.
pub fn read_stage(source: &dyn ThrowSource, library: &MapLibrary, id: &ThrowId) -> Result<Option<StageFacts>, SourceError> {
    let (Some(throw), Some(derived)) = (library.throw(id), library.derived(id)) else { return Ok(None) };
    let Some(mut tracks) = source.tracks(id)? else { return Ok(None) };
    if let Effect::Flash(blinds) = &mut tracks.effect
        && blinds.iter().any(|b| b.pos.is_none())
        && let Some((demo, ticks)) = library.blind_window(id)
    {
        nades_model::derive::place_blinds(throw, blinds, &source.samples(&demo, ticks)?);
    }
    let samples = match library.viewpoint_window(id) {
        Some((demo, ticks)) => source.samples(&demo, ticks)?,
        None => Vec::new(),
    };
    let viewpoints = library.viewpoints(id, &samples);
    Ok(Some(StageFacts { throw: throw.clone(), derived: derived.clone(), demo: library.demo(&throw.demo).cloned(), tracks, viewpoints }))
}

/// A map's throws cut by the filters: the map, and the sandbox's query.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub map: String,
    pub query: LineupQuery,
}

impl Filter {
    pub fn map(map: &str) -> Self {
        Self { map: map.to_string(), query: LineupQuery::default() }
    }
}

/// One map's library, read and built; the same `Arc` until the data changes.
pub fn read_library(source: &dyn ThrowSource, map: &str) -> Result<Arc<MapLibrary>, SourceError> {
    let map = MapName(map.to_string());
    Ok(Arc::new(MapLibrary::new(source.map(&map)?, source.places(&map)?)))
}

/// A collider being built: None until the build is done, then the collider (None when the map has no mesh).
type Slot = Mutex<Option<Option<Arc<Collider>>>>;

/// A map's collider, once built: what stage builds wait for.
#[derive(Clone, Default)]
pub struct SharedCollider(Arc<(Slot, Condvar)>);

impl SharedCollider {
    /// Builds `map`'s collider on its own thread, from its pack.
    pub fn build(map: String, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Self::default();
        let filled = shared.clone();
        std::thread::Builder::new()
            .name("collider".into())
            .spawn(move || {
                let started = Instant::now();
                let collider = crate::assets::collider(&map);
                match &collider {
                    Ok(c) => eprintln!("collider: {map}, {} triangles in {:.1} ms", c.triangles(), started.elapsed().as_secs_f64() * 1e3),
                    Err(e) => eprintln!("collider: {e:#} (result shots without line of sight, the smoke unlit)"),
                }
                let (slot, ready) = &*filled.0;
                *slot.lock().expect("collider slot") = Some(collider.ok().map(Arc::new));
                ready.notify_all();
                wake();
            })
            .expect("a collider thread");
        shared
    }

    /// The collider, waiting for its build at most `timeout`; None when it is not built by then or the map has no mesh.
    pub fn wait_for(&self, timeout: Duration) -> Option<Arc<Collider>> {
        let (slot, ready) = &*self.0;
        let mut built = slot.lock().expect("collider slot");
        let started = Instant::now();
        while built.is_none() && started.elapsed() < timeout {
            (built, _) = ready.wait_timeout(built, Duration::from_millis(100)).expect("collider slot");
        }
        built.clone().flatten()
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
    Prepare { map: String, throws: Vec<ThrowId>, collider: SharedCollider, toward_sun: DVec3 },
    Shutdown,
}

pub enum Reply {
    /// The slice's lineups, with the map's library they come from (for the shell's details).
    List { filter: Filter, library: Arc<MapLibrary>, lineups: Vec<Lineup>, teams: Vec<String>, players: Vec<String>, ms: f64 },
    Prepared { map: String, prepared: Arc<Prepared> },
    Failed(String),
}

/// The data thread's handle: requests go in, replies come out (each with a wake-up).
pub struct Library {
    requests: Sender<Work>,
    pub replies: Receiver<Reply>,
    pub source: Arc<dyn ThrowSource>,
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

/// How long a read waits for a producer to finish writing before giving up.
const PATIENCE: Duration = Duration::from_secs(600);

impl Library {
    pub fn start(source: Arc<dyn ThrowSource>, wake: impl Fn() + Send + Sync + Clone + 'static) -> Self {
        let (requests, inbox) = std::sync::mpsc::channel::<Work>();
        let (outbox, replies) = std::sync::mpsc::channel::<Reply>();
        let shutdown = Arc::new(Shutdown::default());
        let stopping = shutdown.clone();
        let reading = source.clone();
        let worker = std::thread::Builder::new()
            .name("data".into())
            .spawn(move || serve(reading.as_ref(), inbox, outbox, wake, &stopping))
            .expect("a data thread");
        Self { requests, replies, source, worker: Some(worker), shutdown }
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

/// The maps' libraries, each with the revision of the data it was built from.
#[derive(Default)]
struct Libraries(HashMap<String, (Revision, Arc<MapLibrary>)>);

impl Libraries {
    /// `map`'s library for the data as it is now: the one built, or one built afresh once the data changed.
    fn get(&mut self, source: &dyn ThrowSource, map: &str) -> Result<Arc<MapLibrary>, SourceError> {
        let revision = source.revision()?;
        if let Some((built, library)) = self.0.get(map)
            && *built == revision
        {
            return Ok(library.clone());
        }
        let library = read_library(source, map)?;
        self.0.insert(map.to_string(), (revision, library.clone()));
        Ok(library)
    }
}

/// The data thread: lists answered as they come, newest wins; stage reads batched, the newest request first.
fn serve(source: &dyn ThrowSource, inbox: Receiver<Work>, outbox: Sender<Reply>, wake: impl Fn() + Send + Sync + Clone + 'static, shutdown: &Shutdown) {
    let mut queue: VecDeque<Work> = VecDeque::new();
    let mut libraries = Libraries::default();
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
                let read = patiently(&outbox, &wake, shutdown, || libraries.get(source, &filter.map));
                if shutdown.requested() { return; }
                let reply = match read {
                    Ok(library) => {
                        let lineups = library.lineups(&filter.query);
                        let facets = library.facets(filter.query.team.as_deref());
                        Reply::List { filter, library, lineups, teams: facets.teams, players: facets.players, ms: started.elapsed().as_secs_f64() * 1e3 }
                    }
                    Err(e) => Reply::Failed(failure(e)),
                };
                let _ = outbox.send(reply);
                wake();
            }
            Request::Prepare { map, throws, collider, toward_sun } => {
                // in slices, so a newer request (a lineup just picked) is not kept waiting behind a long list
                let mut rest = throws;
                while !rest.is_empty() {
                    if shutdown.requested() { return; }
                    let slice: Vec<ThrowId> = rest.drain(..rest.len().min(8)).collect();
                    let read = patiently(&outbox, &wake, shutdown, || {
                        let library = libraries.get(source, &map)?;
                        let mut rows = Vec::new();
                        for id in &slice {
                            rows.extend(read_stage(source, &library, id)?);
                        }
                        Ok(rows)
                    });
                    if shutdown.requested() { return; }
                    match read {
                        Ok(rows) => {
                            // the stages need the collider; build them on every core, each reply waking the window
                            let collider = collider.wait(shutdown);
                            if shutdown.requested() { return; }
                            for s in rows {
                                let (outbox, wake, collider, map) = (outbox.clone(), wake.clone(), collider.clone(), map.clone());
                                rayon::spawn(move || {
                                    let prepared = Arc::new(Prepared::new(s.throw, s.derived, s.demo, &s.tracks, &s.viewpoints, collider.as_deref(), toward_sun));
                                    let _ = outbox.send(Reply::Prepared { map, prepared });
                                    wake();
                                });
                            }
                        }
                        Err(e) => {
                            let _ = outbox.send(Reply::Failed(failure(e)));
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

/// A read that says so, once, while a producer is writing the data, then waits for it.
fn patiently<T>(outbox: &Sender<Reply>, wake: &impl Fn(), shutdown: &Shutdown, mut read: impl FnMut() -> Result<T, SourceError>) -> Result<T, SourceError> {
    let started = Instant::now();
    let mut announced = false;
    loop {
        if shutdown.requested() { return Err(SourceError::Busy); }
        match read() {
            Err(SourceError::Busy) => {
                if !announced {
                    let _ = outbox.send(Reply::Failed("a producer is writing the throws: the lineups follow when it is done".into()));
                    wake();
                    announced = true;
                }
                if started.elapsed() >= PATIENCE || shutdown.wait(Duration::from_millis(500)) { return Err(SourceError::Busy); }
            }
            other => return other,
        }
    }
}

fn failure(e: SourceError) -> String {
    match e {
        SourceError::Missing(path) => format!("no throws at {} (pass --data)", path.display()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_library_waits_for_its_worker() {
        let (requests, _inbox) = std::sync::mpsc::channel();
        let (_outbox, replies) = std::sync::mpsc::channel();
        let (started, active) = std::sync::mpsc::channel();
        let (release, proceed) = std::sync::mpsc::channel();
        let (finished, native_done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            proceed.recv().unwrap();
            finished.send(()).unwrap();
        });
        let library = Library {
            requests, replies, source: Arc::new(nades_model::fixture::Memory::default()), worker: Some(worker), shutdown: Arc::new(Shutdown::default()),
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
        let stages = |map: &str, throw: &str, priority| Work { request: Request::Prepare { map: map.into(), throws: vec![ThrowId(throw.into())], collider: SharedCollider::default(), toward_sun: DVec3::Z }, priority };
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
                Request::Prepare { throws, .. } => throws[0].0.clone(),
                Request::Shutdown => "shutdown".into(),
            })
            .collect();
        // the shown map's older list is dropped
        assert_eq!(served, ["list de_cache", "picked", "list de_mirage", "nuke first", "bulk"]);
    }
}

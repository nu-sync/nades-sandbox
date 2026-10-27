//! A map's collider (its pack's Collider section: the map's own physics and a BVH over it), built once per map on a
//! thread of its own after the first frame. The simulator's flights and effects wait for it.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nades_scene::collider::Collider;

/// A collider being built: None until the build is done, then the collider (None when the map has none).
type Slot = Mutex<Option<Option<Arc<Collider>>>>;

/// A map's collider, once built.
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
                    Err(e) => eprintln!("collider: {e:#} (grenades fly through walls, smokes fill nothing, the smoke unlit)"),
                }
                let (slot, ready) = &*filled.0;
                *slot.lock().expect("collider slot") = Some(collider.ok().map(Arc::new));
                ready.notify_all();
                wake();
            })
            .expect("a collider thread");
        shared
    }

    /// A collider already built.
    #[cfg(test)]
    pub fn ready(collider: Option<Collider>) -> Self {
        let shared = Self::default();
        *shared.0.0.lock().expect("collider slot") = Some(collider.map(Arc::new));
        shared
    }

    /// Whether the build is done (with or without a collider).
    pub fn built(&self) -> bool {
        self.0.0.lock().expect("collider slot").is_some()
    }

    /// The collider now, if it is built.
    pub fn get(&self) -> Option<Arc<Collider>> {
        self.0.0.lock().expect("collider slot").clone().flatten()
    }

    /// The collider, waiting for its build at most `timeout`; None when it is not built by then or the map has none.
    pub fn wait_for(&self, timeout: Duration) -> Option<Arc<Collider>> {
        let (slot, ready) = &*self.0;
        let mut built = slot.lock().expect("collider slot");
        let started = Instant::now();
        while built.is_none() && started.elapsed() < timeout {
            (built, _) = ready.wait_timeout(built, Duration::from_millis(100)).expect("collider slot");
        }
        built.clone().flatten()
    }
}

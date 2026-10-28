//! A map's colliders (its pack's Collider and GrenadeCollider sections: the map's own physics, and a BVH over each),
//! built once per map on a thread of its own after the first frame. The simulator's flights and effects wait for them.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nades_scene::collider::Collider;

/// A map's two colliders: line of sight, for the effects and the cameras, and what a grenade bounces off (S40).
pub struct Colliders {
    /// The Collider section: the map's occluders as awpy reduces them.
    pub sight: Collider,
    /// The GrenadeCollider section: the physics a grenade meets, unclustered, its grenade clips kept.
    pub flight: Collider,
}

/// The colliders being built: None until the build is done, then the colliders (None when the map has none).
type Slot = Mutex<Option<Option<Arc<Colliders>>>>;

/// A map's colliders, once built.
#[derive(Clone, Default)]
pub struct SharedCollider(Arc<(Slot, Condvar)>);

impl SharedCollider {
    /// Builds `map`'s colliders on their own thread, from its pack.
    pub fn build(map: String, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Self::default();
        let filled = shared.clone();
        std::thread::Builder::new()
            .name("collider".into())
            .spawn(move || {
                let started = Instant::now();
                let collider = crate::assets::colliders(&map);
                match &collider {
                    Ok(c) => eprintln!(
                        "collider: {map}, {} triangles and the grenade's {} in {:.1} ms",
                        c.sight.triangles(),
                        c.flight.triangles(),
                        started.elapsed().as_secs_f64() * 1e3
                    ),
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

    /// Colliders already built.
    #[cfg(test)]
    pub fn ready(collider: Option<Colliders>) -> Self {
        let shared = Self::default();
        *shared.0.0.lock().expect("collider slot") = Some(collider.map(Arc::new));
        shared
    }

    /// Whether the build is done (with or without a collider).
    pub fn built(&self) -> bool {
        self.0.0.lock().expect("collider slot").is_some()
    }

    /// The colliders now, if they are built.
    pub fn get(&self) -> Option<Arc<Colliders>> {
        self.0.0.lock().expect("collider slot").clone().flatten()
    }

    /// The colliders, waiting for their build at most `timeout`; None when they are not built by then or the map has
    /// none.
    pub fn wait_for(&self, timeout: Duration) -> Option<Arc<Colliders>> {
        let (slot, ready) = &*self.0;
        let mut built = slot.lock().expect("collider slot");
        let started = Instant::now();
        while built.is_none() && started.elapsed() < timeout {
            (built, _) = ready.wait_timeout(built, Duration::from_millis(100)).expect("collider slot");
        }
        built.clone().flatten()
    }
}

mod backend;
mod config;
mod video;

use std::sync::Arc;

use backend::{Backend, Snapshot};
pub use config::Config;
use nades_control::{AppState, CameraPose, ControlError};
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, ContentBlock, Implementation, JsonObject, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const INSTRUCTIONS: &str = "This server drives a CS2 grenade simulator: it throws grenades on CS2 maps, flies them through the map with CS2's physics, sets them off (smoke, fire, flash, HE, decoy) and renders images and videos. The person talks to you; you drive it. A throw is a grenade and a spot: a CS2 console string `setpos x y z; setang pitch yaw 0` (positions are the player's feet in Hammer units, Z up; positive pitch looks down) or feet/pitch/yaw, plus movement (stand, walk, run, jumpthrow, run_jumpthrow), click (left = full throw, middle = medium, right = lob), crouch and start (seconds on the scene's clock). A scene is a map (e.g. de_mirage; maps_list lists them) and its throws. Without a window: throws_simulate answers where and when each throw lands and goes off; view_render renders a moment of a scene as a PNG; video_render renders an MP4 with a scripted shot, e.g. shot=[{move:follow,throw:0},{move:orbit,throw:0,radius:350,height:120,seconds:8,degrees:360}] follows the flight then pans 360° around what it left (poll video_status; pass output to choose the file). With a window (app_start opens one showing only the 3D view and returns the instance_id every window tool takes): scene_set (also the way to change map), throw_add, throw_update, throw_remove, scene_get (with what each throw did), time_set, view_set (thrower, follow, orbit, free), camera_set, camera_look_at, frame_capture to see it. app_status (start here) and diagnostics_get report state; app_stop closes the window. No tool builds packs or exports game assets.";

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StatusResponse {
    pub summary: String,
    pub lifecycle: String,
    pub instance_id: Option<String>,
    pub requested_map: Option<String>,
    pub pid: Option<u32>,
    pub state: Option<AppState>,
    pub last_error: Option<ControlError>,
    pub capabilities: Capabilities,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Capabilities {
    pub window_control: bool,
    pub headless_render: bool,
    pub headless_video: bool,
    pub headless_content: Vec<String>,
    pub persistent_headless: bool,
    pub map_switch_strategy: String,
    pub manual_window_attachment: bool,
    pub timeline: bool,
}

impl From<Snapshot> for StatusResponse {
    fn from(snapshot: Snapshot) -> Self {
        let ready = snapshot.lifecycle == "ready";
        let summary = match snapshot.state.as_ref().filter(|_| ready) {
            Some(state) => {
                let pending = if state.presented_revision == state.revision {
                    String::new()
                } else {
                    format!(
                        " Revision {} awaits presentation; frame shows revision {}.",
                        state.revision, state.presented_revision
                    )
                };
                let mut selection = format!(", {} throws", state.scene.throws);
                if let Some(k) = state.scene.selected_throw {
                    selection.push_str(&format!(", throw {k} selected"));
                }
                selection.push_str(&format!(
                    ", {:.3} of {:.3} s {}",
                    state.scene.timeline_seconds,
                    state.scene.timeline_end_seconds,
                    if state.scene.playing { "playing" } else { "paused" }
                ));
                if !state.scene.active_effects.is_empty() {
                    selection.push_str(&format!(
                        ", effects: {}",
                        state.scene.active_effects.join(", ")
                    ));
                }
                if state.scene.loading {
                    selection.push_str(", loading (a map switching in, or throws being simulated)");
                }
                if let Some(notice) = &state.scene.notice {
                    let notice: String = notice
                        .chars()
                        .take(256)
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect();
                    selection.push_str(&format!(", notice: {notice}"));
                }
                format!(
                    "Showing {} from the {} camera{}, {}×{} physical pixels, frame {}, presented revision {}.{}",
                    state.map,
                    state.scene.camera_mode,
                    selection,
                    state.viewport.width,
                    state.viewport.height,
                    state.frame_id,
                    state.presented_revision,
                    pending
                )
            }
            None => match snapshot.requested_map.as_ref() {
                Some(map) => format!(
                    "Application is {} on {map}. No ready managed view is shown.",
                    snapshot.lifecycle
                ),
                None => format!(
                    "Application is {}. No ready managed view is shown.",
                    snapshot.lifecycle
                ),
            },
        };
        let next_actions = if ready
            && snapshot
                .state
                .as_ref()
                .is_some_and(|state| state.scene.loading)
        {
            vec![
                "app_status",
                "diagnostics_get",
                "frame_capture",
                "map_open",
                "app_stop",
            ]
        } else if ready {
            vec![
                "scene_get",
                "throw_add",
                "time_set",
                "view_set",
                "frame_capture",
                "camera_set",
                "app_stop",
            ]
        } else if snapshot.lifecycle == "starting" || snapshot.lifecycle == "stopping" {
            vec!["app_status", "diagnostics_get"]
        } else {
            vec!["maps_list", "throws_simulate", "view_render", "video_render", "app_start"]
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        Self {
            summary,
            lifecycle: snapshot.lifecycle,
            instance_id: snapshot.instance_id,
            requested_map: snapshot.requested_map,
            pid: snapshot.pid,
            state: snapshot.state,
            last_error: snapshot.last_error,
            capabilities: Capabilities {
                window_control: true,
                headless_render: true,
                headless_video: true,
                headless_content: vec!["map".into(), "scene".into()],
                persistent_headless: false,
                map_switch_strategy: "restart".into(),
                manual_window_attachment: false,
                timeline: true,
            },
            next_actions,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Failure {
    pub summary: String,
    pub error: ControlError,
    pub status: StatusResponse,
}

#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
enum ToolResponse<T> {
    Success(T),
    Failure(Box<Failure>),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MapOpenResponse {
    #[serde(flatten)]
    pub status: StatusResponse,
    pub restarted: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImageInfo {
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub include_ui: bool,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CaptureResponse {
    pub summary: String,
    pub state: AppState,
    pub image: ImageInfo,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RenderResponse {
    pub summary: String,
    pub map: String,
    /// The camera given; none when a view of a throw chose it.
    pub camera: Option<CameraPose>,
    pub image: ImageInfo,
    pub render_time_seconds: f64,
    pub startup_marks: Option<serde_json::Value>,
    /// What each throw of the scene did: its release, bounces, where and when it went off, its effect.
    pub throws: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SceneResponse {
    #[serde(flatten)]
    pub status: StatusResponse,
    pub scene: nades_control::scene::Scene,
    /// What each simulated throw did (release, bounces, landing, detonation, its effect), in the scene's order; a throw
    /// still being simulated is missing until it is (status shows loading).
    pub reports: Vec<nades_control::scene::ThrowReport>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SimulateResponse {
    pub summary: String,
    pub map: String,
    pub reports: Vec<nades_control::scene::ThrowReport>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MapInfo {
    pub map: String,
    pub status: String,
    pub pack_version: Option<u32>,
    pub bytes: u64,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MapsResponse {
    pub summary: String,
    pub pack_format: u32,
    pub maps: Vec<MapInfo>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DiagnosticsResponse {
    #[serde(flatten)]
    pub status: StatusResponse,
    pub build_version: String,
    pub logs: Vec<String>,
    pub log_path: Option<String>,
    pub metric_definitions: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoJobParams {
    pub job_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartParams {
    /// Locally packed CS2 map ID, such as de_cache. No path or extension.
    #[serde(default = "default_map")]
    pub map: String,
}
fn default_map() -> String {
    "de_cache".into()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceParams {
    /// Instance ID returned by app_start or the most recent map_open.
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapOpenParams {
    pub instance_id: String,
    pub map: String,
    /// Optional initial pose. Otherwise use the application's existing default camera.
    pub camera: Option<CameraPose>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CameraParams {
    pub instance_id: String,
    pub camera: CameraPose,
    /// Reject if user input or another operation has since changed the view.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureParams {
    pub instance_id: String,
    /// Include the UI overlay. False captures the scene from the same frame before that overlay.
    #[serde(default = "yes")]
    pub include_ui: bool,
    /// Exact view revision to capture; reject instead of silently capturing another view.
    pub expected_revision: Option<u64>,
}
pub(crate) fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneSetParams {
    pub instance_id: String,
    /// The map and its throws; each throw's spot may be a console string (setpos …; setang …).
    pub scene: nades_control::scene::SceneInput,
    /// Reject if user input or another operation has since changed the view.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThrowAddParams {
    pub instance_id: String,
    pub throw: nades_control::scene::ThrowInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimulateParams {
    /// The map and its throws; each throw's spot may be a console string (setpos …; setang …).
    pub scene: nades_control::scene::SceneInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LookAtParams {
    pub instance_id: String,
    /// Where the camera's eye is, Hammer units (Z up).
    pub from: [f32; 3],
    /// The point it looks at.
    pub at: [f32; 3],
    /// Vertical field of view in degrees (CS2's 73.74 by default).
    #[serde(default = "nades_control::default_fov")]
    pub vertical_fov_degrees: f32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoParams {
    /// The map and its throws; each throw's spot may be a console string (setpos …; setang …).
    pub scene: nades_control::scene::SceneInput,
    /// Seconds on the scene's clock where the clip starts (default 0; each throw begins at its own start).
    #[serde(default)]
    pub start_seconds: f64,
    /// Where it ends; with a shot, omit it and the clip ends with the shot.
    #[serde(default)]
    pub end_seconds: Option<f64>,
    /// Even, at most 1920 (default 1280).
    #[serde(default)]
    pub width: Option<u32>,
    /// Even, at most 1080 (default 720).
    #[serde(default)]
    pub height: Option<u32>,
    /// 1–60 (default 30); at most 1800 frames in all.
    #[serde(default)]
    pub fps: Option<u32>,
    /// A scripted camera: moves one after another, the scene's clock running with them. E.g. follow the flight, then a
    /// 360° pan around what it left: [{"move":"follow","throw":0},{"move":"orbit","throw":0,"radius":350,"height":120,"seconds":8,"degrees":360}].
    #[serde(default)]
    pub shot: Vec<nades_control::video::ShotMove>,
    /// Instead of a shot: a fixed camera.
    #[serde(default)]
    pub camera: Option<CameraPose>,
    /// Instead of a shot: timed camera poses (seconds from the clip's start; the first at 0).
    #[serde(default)]
    pub camera_keyframes: Vec<nades_control::video::CameraKeyframe>,
    /// Instead of a shot or camera: thrower, follow or orbit of `throw` for the whole clip.
    #[serde(default)]
    pub view: Option<nades_control::scene::ViewName>,
    #[serde(default)]
    pub throw: Option<usize>,
    /// An absolute path ending in .mp4 to write the finished video to (its folder must exist); else the MCP's runs
    /// folder. video_status gives the path.
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThrowUpdateParams {
    pub instance_id: String,
    /// The throw's index in the scene.
    pub index: usize,
    pub throw: nades_control::scene::ThrowInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThrowRemoveParams {
    pub instance_id: String,
    /// The throw's index in the scene.
    pub index: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeParams {
    pub instance_id: String,
    /// Seconds on the scene's clock; omit to keep the moment.
    pub seconds: Option<f64>,
    /// Play or pause; omit to pause when seconds are given.
    pub playing: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewParams {
    pub instance_id: String,
    pub view: nades_control::scene::ViewName,
    /// The throw's index (the selected one when omitted).
    pub throw: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderParams {
    pub map: String,
    /// A CS2 camera; may be omitted when a scene's throw view chooses it.
    pub camera: Option<CameraPose>,
    /// A scene to draw on the map at a moment, with the view to see it from.
    pub scene: Option<backend::SceneShot>,
    /// Physical image width, 1–4096 pixels, within GPU limits.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Physical image height, 1–4096 pixels, within GPU limits.
    #[serde(default = "default_height")]
    pub height: u32,
    /// Material animation timestamp in seconds. Copy capture metadata to reproduce its animation phase.
    #[serde(default)]
    pub render_time_seconds: f64,
}
fn default_width() -> u32 {
    1138
}
fn default_height() -> u32 {
    640
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsParams {
    /// Recent log records, default 50, maximum 200.
    #[serde(default = "default_log_limit")]
    pub limit: usize,
}
fn default_log_limit() -> usize {
    50
}

#[derive(Clone)]
pub struct NadesMcp {
    backend: Backend,
    config: Config,
}

impl NadesMcp {
    pub fn new(config: Config) -> Self {
        Self {
            backend: Backend::new(config.clone()),
            config,
        }
    }

    pub async fn detach(&self) {
        self.backend.detach().await;
    }

    async fn fail(&self, error: ControlError) -> CallToolResult {
        CallToolResult::structured_error(
            serde_json::to_value(ToolResponse::<StatusResponse>::Failure(Box::new(Failure {
                summary: error.message.clone(),
                error,
                status: self.backend.status().await.into(),
            })))
            .expect("finite tool failure"),
        )
    }

    async fn state_result(&self, result: Result<AppState, ControlError>) -> CallToolResult {
        match result {
            Ok(state) => success(self.status_with_state(state).await),
            Err(error) => self.fail(error).await,
        }
    }

    /// Puts `scene` up in the window, and answers with the scene as the window then has it.
    async fn apply_scene(&self, instance_id: &str, scene: nades_control::scene::Scene, expected_revision: Option<u64>) -> CallToolResult {
        if let Err(e) = self.backend.command(instance_id, nades_control::AppCommand::SetScene { scene, expected_revision }).await {
            return self.fail(e).await;
        }
        self.scene_result(instance_id).await
    }

    /// The window's scene, what its throws did and the state, waiting (up to 30 s) until every throw is simulated.
    async fn scene_result(&self, instance_id: &str) -> CallToolResult {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match self.backend.scene(instance_id).await {
                Ok((state, scene, reports)) if reports.len() == scene.throws.len() || tokio::time::Instant::now() >= deadline => {
                    return success(SceneResponse { status: self.status_with_state(state).await, scene, reports });
                }
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                Err(e) => return self.fail(e).await,
            }
        }
    }

    /// Reads the window's scene, changes it with `change`, and puts it back.
    /// The view's revision also moves when a simulation finishes or the person flies the camera, so a write that finds
    /// it moved is tried again on a fresh read, as long as the scene itself is what was read.
    async fn edit_scene(&self, instance_id: &str, change: impl Fn(&mut nades_control::scene::Scene) -> Result<(), ControlError>) -> CallToolResult {
        let mut first: Option<nades_control::scene::Scene> = None;
        for _ in 0..5 {
            let (state, read, _) = match self.backend.scene(instance_id).await {
                Ok(read) => read,
                Err(e) => return self.fail(e).await,
            };
            if first.get_or_insert_with(|| read.clone()) != &read {
                return self.fail(ControlError::new("stale_revision", "The scene changed while it was being edited; read it again with scene_get.", true)).await;
            }
            let mut scene = read;
            let changed = match change(&mut scene).and_then(|()| scene.validated()) {
                Ok(scene) => scene,
                Err(e) => return self.fail(e).await,
            };
            match self.backend.command(instance_id, nades_control::AppCommand::SetScene { scene: changed, expected_revision: Some(state.revision) }).await {
                Ok(_) => return self.scene_result(instance_id).await,
                Err(e) if e.code == "stale_revision" => continue,
                Err(e) => return self.fail(e).await,
            }
        }
        self.fail(ControlError::new("stale_revision", "The view kept changing while the scene was being edited; try again.", true)).await
    }

    async fn status_with_state(&self, state: AppState) -> StatusResponse {
        let mut snapshot = self.backend.status().await;
        if snapshot.instance_id.as_deref() == Some(state.instance_id.as_str()) {
            snapshot.state = Some(state);
        }
        snapshot.into()
    }
}

#[tool_router]
impl NadesMcp {
    #[tool(description = "Observe the managed app without starting it. Returns lifecycle, what is shown, CS2 camera, viewport, presented frame/revision, explicitly labelled CPU timings, capabilities and suggested next tools. Start here.", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn app_status(&self) -> CallToolResult {
        success(StatusResponse::from(self.backend.status().await))
    }

    #[tool(description = "Open an agent-owned viewer window on a local packed map (default de_cache). Waits for a presented frame AND a visible window. Reuses the same running map; use map_open for another map. On MCP disconnect the ready window stays open for the user.", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false))]
    pub async fn app_start(
        &self,
        Parameters(params): Parameters<StartParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.state_result(cancellable(&context, self.backend.start(&params.map)).await)
            .await
    }

    #[tool(description = "Close only the specified agent-owned window and confirm process exit. Repeated stop of the last stopped instance is harmless. Never closes manually opened or previously handed-off windows.", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false))]
    pub async fn app_stop(
        &self,
        Parameters(params): Parameters<InstanceParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        match cancellable(&context, self.backend.stop(&params.instance_id)).await {
            Ok(snapshot) => success(StatusResponse::from(snapshot)),
            Err(error) => self.fail(error).await,
        }
    }

    #[tool(description = "Discover local map packs and their usable/incompatible/corrupt status. Sorted stable map IDs; no game assets are returned and no packs are built. An unavailable pack can be built separately with just pack MAP.", output_schema = output_schema::<MapsResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn maps_list(&self) -> CallToolResult {
        match list_maps(&self.config.packs_dir) {
            Ok(maps) => {
                let usable = maps.iter().filter(|entry| entry.status == "usable").count();
                success(MapsResponse {
                    summary: format!(
                        "{usable} usable map packs out of {} local files.",
                        maps.len()
                    ),
                    pack_format: nades_pack::VERSION,
                    maps,
                    next_actions: vec!["app_start".into(), "view_render".into()],
                })
            }
            Err(error) => self.fail(error).await,
        }
    }

    #[tool(description = "Change the managed window's map. Validate the local pack and optional camera first; another map restarts the owned app and returns a NEW instance_id with restarted=true. Invalid input preserves the current window. Same-map requests avoid restart.", output_schema = output_schema::<MapOpenResponse>(), annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false))]
    pub async fn map_open(
        &self,
        Parameters(params): Parameters<MapOpenParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        match cancellable(
            &context,
            self.backend
                .open_map(&params.instance_id, &params.map, params.camera),
        )
        .await
        {
            Ok((state, restarted)) => success(MapOpenResponse {
                status: self.status_with_state(state).await,
                restarted,
            }),
            Err(error) => self.fail(error).await,
        }
    }

    #[tool(description = "Set an absolute camera and wait for its frame to be presented. Position is CS2 player FEET in Hammer units, Z up; pitch/yaw are degrees, positive pitch down. Eye height defaults 64, vertical FOV 73.7398°. Returns actual pose/revision. Follow with frame_capture using expected_revision.", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false))]
    pub async fn camera_set(
        &self,
        Parameters(params): Parameters<CameraParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.state_result(
            cancellable(
                &context,
                self.backend.set_camera(
                    &params.instance_id,
                    params.camera,
                    params.expected_revision,
                ),
            )
            .await,
        )
        .await
    }

    #[tool(description = "See the managed window: returns a native PNG image plus exact map, camera, revision, frame, physical dimensions and animation timestamp. include_ui defaults true; false returns only the scene viewport, cropped before the overlay in the same submission. Does not close the app. expected_revision rejects an intervening view change.", output_schema = output_schema::<CaptureResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn frame_capture(
        &self,
        Parameters(params): Parameters<CaptureParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        match cancellable(
            &context,
            self.backend.capture(
                &params.instance_id,
                params.include_ui,
                params.expected_revision,
            ),
        )
        .await
        {
            Ok((state, png, width, height)) => {
                let metadata = CaptureResponse {
                    summary: format!(
                        "Captured {} frame {} at revision {}.",
                        state.map, state.frame_id, state.revision
                    ),
                    image: image_info(width, height, params.include_ui, &png),
                    state,
                };
                with_image(metadata, png)
            }
            Err(error) => self.fail(error).await,
        }
    }

    #[tool(description = "Render headlessly without opening or changing any visible window: the map from a CS2 camera, or a scene (its map and throws, simulated) at time_seconds on its clock, from a view of one of its throws (orbit, the default: a result shot over what it left; follow: behind the grenade; thrower: the thrower's eye) or from the camera. Size defaults 1138×640, max 4096 per side. Returns a native PNG and what each throw did (release, bounces, where and when it went off, its smoke or fire).", output_schema = output_schema::<RenderResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn view_render(
        &self,
        Parameters(params): Parameters<RenderParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        match cancellable(
            &context,
            self.backend.render(
                &params.map,
                params.camera,
                params.width,
                params.height,
                params.render_time_seconds,
                params.scene,
            ),
        )
        .await
        {
            Ok(rendered) => {
                let metadata = RenderResponse {
                    summary: format!(
                        "Rendered {}{} headlessly at {}×{} pixels; visible window unchanged.",
                        rendered.map,
                        if rendered.throws.is_empty() { String::new() } else { format!(" with {} throws", rendered.throws.len()) },
                        rendered.width,
                        rendered.height
                    ),
                    image: image_info(rendered.width, rendered.height, false, &rendered.png_base64),
                    map: rendered.map,
                    camera: rendered.camera,
                    render_time_seconds: rendered.render_time_seconds,
                    startup_marks: rendered.startup_marks,
                    throws: rendered.throws,
                };
                with_image(metadata, rendered.png_base64)
            }
            Err(error) => self.fail(error).await,
        }
    }

    #[tool(description = "Render a video of a scene to MP4, headlessly (no window): its throws simulated and played with the scene's clock. The camera is a `shot`, a list of moves played one after another: {\"move\":\"follow\",\"throw\":k,\"after_seconds\":s} trails the grenade until it goes off (plus s seconds); {\"move\":\"orbit\",\"throw\":k,\"radius\":350,\"height\":120,\"seconds\":8,\"degrees\":360} circles horizontally around what the throw left (its smoke, fire or burst), looking at it, starting from the camera's current bearing and easing in; {\"move\":\"thrower\",\"throw\":k,\"seconds\":2} is the thrower's eye; {\"move\":\"hold\",\"camera\":{…},\"seconds\":2} a fixed camera. E.g. follow the flight then a 360° pan around the smoke: shot=[{\"move\":\"follow\",\"throw\":0},{\"move\":\"orbit\",\"throw\":0}]. With a shot, end_seconds may be left out. Alternatively a fixed camera, camera_keyframes or a view of a throw for the whole clip. Give `output` (an absolute .mp4 path) to choose where it is written. Returns a job at once: poll video_status until completed for the path and what each throw did; video_cancel stops it.", output_schema = output_schema::<video::VideoStatus>(), annotations(read_only_hint = false, destructive_hint = false, open_world_hint = false))]
    pub async fn video_render(
        &self,
        Parameters(params): Parameters<VideoParams>,
    ) -> CallToolResult {
        let scene = match params.scene.scene() {
            Ok(scene) => scene,
            Err(e) => return self.fail(e).await,
        };
        let request = nades_control::video::VideoRequest {
            scene,
            start_seconds: params.start_seconds,
            end_seconds: params.end_seconds,
            width: params.width.unwrap_or(1280),
            height: params.height.unwrap_or(720),
            fps: params.fps.unwrap_or(30),
            camera: params.camera,
            camera_keyframes: params.camera_keyframes,
            view: params.view,
            throw: params.throw,
            shot: params.shot,
        };
        match self.backend.video_start(request, params.output.map(std::path::PathBuf::from)).await {
            Ok(v) => success(v),
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(description = "Simulate throws headlessly (no window, no GPU, a second or two) and report what each did: when and where it leaves the hand and its velocity, every bounce with the surface's normal, where it first lands on a floor, when and where it goes off (times in seconds on the scene's clock, from the throw's start), how long its effect lasts and where to look at it (the smoke's middle, the fire), and the practice command. A throw's spot is a CS2 console string, e.g. {\"grenade\":\"smoke\",\"console\":\"setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0\",\"movement\":\"jumpthrow\",\"click\":\"left\",\"crouch\":false}, or feet, pitch and yaw. Use it to answer where and when a lineup lands before rendering anything.", output_schema = output_schema::<SimulateResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn throws_simulate(&self, Parameters(params): Parameters<SimulateParams>) -> CallToolResult {
        let scene = match params.scene.scene() {
            Ok(scene) => scene,
            Err(e) => return self.fail(e).await,
        };
        let map = scene.map.clone();
        match self.backend.simulate(scene).await {
            Ok(reports) => {
                let lines: Vec<String> = reports.iter().map(|r| format!("throw {} ({:?}) goes off at {:.2} s at ({:.0}, {:.0}, {:.0})", r.throw, r.grenade, r.detonation.t, r.detonation.at[0], r.detonation.at[1], r.detonation.at[2])).collect();
                success(SimulateResponse { summary: format!("Simulated {} throws on {map}: {}.", reports.len(), lines.join("; ")), map, reports })
            }
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(description = "Point the window's free camera from one point at another (Hammer units, Z up): e.g. from above a smoke's detonation, looking at it. Same as camera_set with the angles worked out; returns the applied camera and revision for frame_capture.", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false))]
    pub async fn camera_look_at(&self, Parameters(params): Parameters<LookAtParams>) -> CallToolResult {
        let d = [params.at[0] - params.from[0], params.at[1] - params.from[1], params.at[2] - params.from[2]];
        let flat = d[0].hypot(d[1]);
        if flat < 1e-3 && d[2].abs() < 1e-3 {
            return self.fail(ControlError::new("invalid_camera", "from and at are the same point.", false)).await;
        }
        let camera = CameraPose { position: params.from, pitch_degrees: (-d[2].atan2(flat).to_degrees()).clamp(-89.9, 89.9), yaw_degrees: d[1].atan2(d[0]).to_degrees(), eye_height_units: 0.0, vertical_fov_degrees: params.vertical_fov_degrees };
        self.state_result(self.backend.set_camera(&params.instance_id, camera, None).await).await
    }

    #[tool(description = "Read the window's scene: its map, every throw placed on it in order (their indexes are what throw_update, throw_remove, view_set and shots take), and what each did (release, bounces, landing, detonation, effect).", output_schema = output_schema::<SceneResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn scene_get(&self, Parameters(params): Parameters<InstanceParams>) -> CallToolResult {
        self.scene_result(&params.instance_id).await
    }

    #[tool(description = "Replace the window's scene: a map and its throws (each a grenade with a console string `setpos …; setang …` or feet/pitch/yaw, movement, click, crouch, start). A scene on another map switches the window to that map in place (this is how to choose the map). Waits until the throws are simulated and returns what each did. An empty throws list clears the scene.", output_schema = output_schema::<SceneResponse>(), annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false))]
    pub async fn scene_set(&self, Parameters(params): Parameters<SceneSetParams>) -> CallToolResult {
        let scene = match params.scene.scene() {
            Ok(scene) => scene,
            Err(e) => return self.fail(e).await,
        };
        self.apply_scene(&params.instance_id, scene, params.expected_revision).await
    }

    #[tool(description = "Place a throw in the window's scene and select it. The spot is a CS2 console string (`console`: \"setpos x y z; setang pitch yaw 0\", as getpos prints it) or feet (Hammer units, Z up) with pitch and yaw (degrees, positive pitch looks down). Also grenade (smoke, flash, he, molotov, incendiary, decoy), movement (stand, walk, run, jumpthrow, run_jumpthrow), click (left full throw, middle medium, right lob), crouch, and start (seconds on the scene's clock). Waits until it is simulated and returns the scene with what each throw did.", output_schema = output_schema::<SceneResponse>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false))]
    pub async fn throw_add(&self, Parameters(params): Parameters<ThrowAddParams>) -> CallToolResult {
        self.edit_scene(&params.instance_id, |scene| {
            scene.throws.push(params.throw.clone().throw()?);
            Ok(())
        })
        .await
    }

    #[tool(description = "Replace throw `index` of the window's scene with another (same fields as throw_add): re-aim it, move it, or change its grenade, movement, crouch, click or start.", output_schema = output_schema::<SceneResponse>(), annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false))]
    pub async fn throw_update(&self, Parameters(params): Parameters<ThrowUpdateParams>) -> CallToolResult {
        self.edit_scene(&params.instance_id, |scene| {
            let count = scene.throws.len();
            let slot = scene.throws.get_mut(params.index).ok_or_else(|| ControlError::new("invalid_throw", format!("The scene has {count} throws."), false))?;
            *slot = params.throw.clone().throw()?;
            Ok(())
        })
        .await
    }

    #[tool(description = "Remove throw `index` from the window's scene; later throws move down one index.", output_schema = output_schema::<SceneResponse>(), annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false))]
    pub async fn throw_remove(&self, Parameters(params): Parameters<ThrowRemoveParams>) -> CallToolResult {
        self.edit_scene(&params.instance_id, |scene| {
            if params.index >= scene.throws.len() {
                return Err(ControlError::new("invalid_throw", format!("The scene has {} throws.", scene.throws.len()), false));
            }
            scene.throws.remove(params.index);
            Ok(())
        })
        .await
    }

    #[tool(description = "Move the window's scene clock to `seconds`, and play or pause it. Each throw begins at its own start; status reports the clock, its end and the effects showing.", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false))]
    pub async fn time_set(&self, Parameters(params): Parameters<TimeParams>) -> CallToolResult {
        let result = self.backend.command(&params.instance_id, nades_control::AppCommand::SetTime { seconds: params.seconds, playing: params.playing }).await;
        self.state_result(result.and_then(state_of)).await
    }

    #[tool(description = "Show a view of a throw in the window: thrower (the throw's eye as it lets go, the clock at its release), follow (80 units behind the grenade, the clock playing from the throw's start), orbit (a shot that sees what it left, the clock at its fullest; drag-free), or free (the camera camera_set and camera_look_at move). `throw` is its index (the selected one by default).", output_schema = output_schema::<StatusResponse>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false))]
    pub async fn view_set(&self, Parameters(params): Parameters<ViewParams>) -> CallToolResult {
        let result = self.backend.command(&params.instance_id, nades_control::AppCommand::SetView { view: params.view, throw: params.throw }).await;
        self.state_result(result.and_then(state_of)).await
    }

    #[tool(description = "Inspect a video job's progress, failure, and finished local MP4 path.", output_schema = output_schema::<video::VideoStatus>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn video_status(
        &self,
        Parameters(params): Parameters<VideoJobParams>,
    ) -> CallToolResult {
        match self.backend.video_status(&params.job_id).await {
            Ok(v) => success(v),
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(description = "Cancel an in-progress video job; poll video_status until it reports cancelled.", output_schema = output_schema::<video::VideoStatus>(), annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false))]
    pub async fn video_cancel(
        &self,
        Parameters(params): Parameters<VideoJobParams>,
    ) -> CallToolResult {
        match self.backend.video_cancel(&params.job_id).await {
            Ok(v) => success(v),
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(description = "Inspect build/GPU information, actual startup stage timings, labelled frame CPU metrics and recent local logs. Default 50 log records, maximum 200. Null GPU timings mean unmeasured, not zero. No base64 image or game assets are returned. Useful after errors and slow launches.", output_schema = output_schema::<DiagnosticsResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn diagnostics_get(
        &self,
        Parameters(params): Parameters<DiagnosticsParams>,
    ) -> CallToolResult {
        if params.limit > 200 {
            return self
                .fail(ControlError::new(
                    "invalid_limit",
                    "Request at most 200 log records.",
                    false,
                ))
                .await;
        }
        let snapshot = self.backend.status().await;
        let log_path = snapshot.log_path.clone();
        success(DiagnosticsResponse {
            status: snapshot.into(), build_version: env!("CARGO_PKG_VERSION").into(), logs: self.backend.logs(params.limit).await, log_path,
            metric_definitions: vec!["metrics.*_cpu_ms: CPU wall time preparing and submitting recent frames; not GPU time and not display refresh interval.".into(), "metrics.samples: sample count of the bounded recent-frame window; an idle viewer does not continuously redraw.".into(), "metrics.gpu_ms: null when GPU timing was not measured; no FPS or GPU performance conclusion follows from CPU submission timings.".into(), "metrics.memory: measured process physical footprint and peak, in bytes; graphics_ledger_bytes is a portion of the footprint, not total GPU allocation. Null means unsupported.".into(), "startup_marks: milliseconds since main; pre_main_ms is separate, and the first launch of a new binary may include macOS executable checks.".into()],
        })
    }
}

#[tool_handler]
impl ServerHandler for NadesMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "nades-sandbox",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

pub async fn serve(config: Config) -> anyhow::Result<()> {
    let service = NadesMcp::new(config);
    let lifecycle = service.clone();
    let server = service.serve(rmcp::transport::stdio()).await?;
    let result = server.waiting().await;
    lifecycle.detach().await;
    result?;
    Ok(())
}

fn state_of(output: nades_control::ControlOutput) -> Result<AppState, ControlError> {
    match output {
        nades_control::ControlOutput::State { state } | nades_control::ControlOutput::Scene { state, .. } | nades_control::ControlOutput::Capture { state, .. } => Ok(state),
        nades_control::ControlOutput::Stopped => Err(ControlError::new("protocol_error", "The app stopped.", false)),
    }
}

async fn cancellable<T>(
    context: &RequestContext<RoleServer>,
    operation: impl std::future::Future<Output = Result<T, ControlError>>,
) -> Result<T, ControlError> {
    tokio::select! {
        biased;
        _ = context.ct.cancelled() => Err(ControlError::new("cancelled", "Operation cancelled. Inspect app_status; an already-applied mutation is not rolled back.", false)),
        result = operation => result,
    }
}

fn output_schema<T: JsonSchema>() -> Arc<JsonObject> {
    let mut schema = serde_json::to_value(schemars::schema_for!(ToolResponse<T>))
        .expect("tool output schema")
        .as_object()
        .expect("object schema")
        .clone();
    schema.insert("type".into(), serde_json::Value::String("object".into()));
    schema.into()
}

fn success<T: Serialize>(value: T) -> CallToolResult {
    CallToolResult::structured(
        serde_json::to_value(ToolResponse::Success(value)).expect("finite tool result"),
    )
}

fn with_image(value: impl Serialize, png: String) -> CallToolResult {
    let mut result = success(value);
    result.content.push(ContentBlock::image(png, "image/png"));
    result
}

fn image_info(width: u32, height: u32, include_ui: bool, png: &str) -> ImageInfo {
    let padding = png.bytes().rev().take_while(|byte| *byte == b'=').count();
    ImageInfo {
        mime_type: "image/png".into(),
        width,
        height,
        include_ui,
        bytes: png.len() / 4 * 3 - padding,
    }
}

fn list_maps(directory: &std::path::Path) -> Result<Vec<MapInfo>, ControlError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(ControlError::new(
                "pack_cache_unreadable",
                format!("Cannot read {}: {error}", directory.display()),
                true,
            ));
        }
    };
    let mut maps = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| ControlError::new("pack_cache_unreadable", error.to_string(), true))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("pack") {
            continue;
        }
        let Some(map) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if nades_control::validate_map_id(map).is_err() {
            continue;
        }
        let bytes = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let (status, pack_version, issue) = match nades_pack::Pack::open(&path) {
            Ok(pack) => {
                let result = pack.scene().map(|_| ()).and_then(|()| {
                    if pack.contents().source.map == map {
                        Ok(())
                    } else {
                        Err(nades_pack::PackError::Scene(
                            "map ID does not match pack filename".into(),
                        ))
                    }
                });
                match result {
                    Ok(()) => ("usable", Some(nades_pack::VERSION), None),
                    Err(error) => (
                        "corrupt",
                        Some(nades_pack::VERSION),
                        Some(error.to_string()),
                    ),
                }
            }
            Err(nades_pack::PackError::Version { found }) => (
                "incompatible",
                Some(found),
                Some(format!(
                    "This build reads pack format {}; rebuild with just pack {map}.",
                    nades_pack::VERSION
                )),
            ),
            Err(error) => ("corrupt", None, Some(error.to_string())),
        };
        maps.push(MapInfo {
            map: map.into(),
            status: status.into(),
            pack_version,
            bytes,
            issue,
        });
    }
    maps.sort_by(|a, b| a.map.cmp(&b.map));
    Ok(maps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_corrupt_pack_entries_are_reported_without_assets() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            list_maps(&directory.path().join("missing"))
                .unwrap()
                .is_empty()
        );
        std::fs::write(directory.path().join("de_z.pack"), b"broken").unwrap();
        std::fs::write(directory.path().join("de_a.pack"), b"also broken").unwrap();
        std::fs::write(directory.path().join("unrelated.txt"), b"ignored").unwrap();
        let actual = list_maps(directory.path()).unwrap();
        assert_eq!(
            actual
                .iter()
                .map(|entry| (entry.map.as_str(), entry.status.as_str()))
                .collect::<Vec<_>>(),
            vec![("de_a", "corrupt"), ("de_z", "corrupt")]
        );
    }

    #[test]
    fn image_results_keep_base64_out_of_structured_metadata() {
        let result = with_image(
            serde_json::json!({"summary":"a frame","width":1}),
            "cG5n".into(),
        );
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({"summary":"a frame","width":1}))
        );
        let image = result.content.last().unwrap().as_image().unwrap();
        assert_eq!(image.data, "cG5n");
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image_info(1, 1, false, "aA==").bytes, 1);
    }

    #[tokio::test]
    async fn protocol_discovers_tools_and_validates_status_and_failure_schemas() {
        let directory = tempfile::tempdir().unwrap();
        let service = NadesMcp::new(Config {
            sandbox_binary: directory.path().join("missing-binary"),
            packs_dir: directory.path().join("packs"),
            runs_dir: directory.path().join("runs"),
        });
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let running = service.serve(server_io).await.unwrap();
            running.waiting().await.unwrap();
        });
        let client = ().serve(client_io).await.unwrap();
        let tools = client.list_all_tools().await.unwrap();
        let mut names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "app_start",
                "app_status",
                "app_stop",
                "camera_look_at",
                "camera_set",
                "diagnostics_get",
                "frame_capture",
                "map_open",
                "maps_list",
                "scene_get",
                "scene_set",
                "throw_add",
                "throw_remove",
                "throw_update",
                "throws_simulate",
                "time_set",
                "video_cancel",
                "video_render",
                "video_status",
                "view_render",
                "view_set"
            ]
        );

        let status = client
            .call_tool(rmcp::model::CallToolRequestParams::new("app_status"))
            .await
            .unwrap();
        let value = status.structured_content.unwrap();
        assert_eq!(value["lifecycle"], "stopped");
        assert_eq!(value["state"], serde_json::Value::Null);
        let schema = serde_json::Value::Object(
            (**tools
                .iter()
                .find(|tool| tool.name == "app_status")
                .unwrap()
                .output_schema
                .as_ref()
                .unwrap())
            .clone(),
        );
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&value)
            .unwrap();

        let failed = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("app_start")
                    .with_arguments(serde_json::Map::new()),
            )
            .await
            .unwrap();
        assert_eq!(failed.is_error, Some(true));
        let failure = failed.structured_content.unwrap();
        assert_eq!(failure["error"]["code"], "pack_missing");
        let schema = serde_json::Value::Object(
            (**tools
                .iter()
                .find(|tool| tool.name == "app_start")
                .unwrap()
                .output_schema
                .as_ref()
                .unwrap())
            .clone(),
        );
        jsonschema::validator_for(&schema)
            .unwrap()
            .validate(&failure)
            .unwrap();

        client.cancel().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

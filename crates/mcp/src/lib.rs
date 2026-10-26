mod backend;
mod config;

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

const INSTRUCTIONS: &str = "Operate the native CS2 map viewer through observe → act → verify. Begin with app_status to learn what is actually shown and maps_list to discover local packs. app_start opens one agent-owned window and returns only after a map frame is presented and the window visible. Use the returned instance_id for subsequent operations; map_open may restart the app and returns a replacement instance_id. Set an absolute CS2 camera with camera_set, then use frame_capture with its revision to inspect the rendered result. Coordinates are player feet in Hammer units, Z up; pitch/yaw are degrees and positive pitch looks down. Eye height defaults to 64 units, vertical FOV to 73.7398°. Status and diagnostics expose explicitly labelled CPU timings; null GPU timings mean unavailable, not zero. view_render makes a headless base-map screenshot without launching or changing the visible window. It can reproduce a map-only capture using the same map, camera, image dimensions and render_time_seconds; selected throws and grenade effects are not yet supported headlessly. app_stop explicitly closes the owned window. Disconnect leaves an already-ready window open for the user; later MCP sessions do not reconnect to it. No tool builds packs, edits the database, or exports game assets. M3 lineups, grenade effects and timeline controls are outside this tool set.";

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
                let mut selection = state
                    .scene
                    .lineup_id
                    .as_ref()
                    .map(|id| format!(", lineup {id}"))
                    .unwrap_or_default();
                if let Some(id) = &state.scene.throw_id {
                    selection.push_str(&format!(", throw {id}"));
                }
                if let Some(time) = state.scene.timeline_seconds {
                    selection.push_str(&format!(
                        ", {time:.3}s {}",
                        if state.scene.playing {
                            "playing"
                        } else {
                            "paused"
                        }
                    ));
                }
                if !state.scene.active_effects.is_empty() {
                    selection.push_str(&format!(
                        ", effects: {}",
                        state.scene.active_effects.join(", ")
                    ));
                }
                if state.scene.loading {
                    selection.push_str(", selected scene loading; the map remains available");
                }
                if let Some(notice) = &state.scene.notice {
                    let notice: String = notice
                        .chars()
                        .take(256)
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect();
                    selection.push_str(&format!(", data notice: {notice}"));
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
                "camera_set",
                "frame_capture",
                "map_open",
                "diagnostics_get",
                "app_stop",
            ]
        } else if snapshot.lifecycle == "starting" || snapshot.lifecycle == "stopping" {
            vec!["app_status", "diagnostics_get"]
        } else {
            vec!["maps_list", "app_start", "view_render", "diagnostics_get"]
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
                headless_content: vec!["map".into()],
                persistent_headless: false,
                map_switch_strategy: "restart".into(),
                manual_window_attachment: false,
                timeline: false,
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
    pub camera: CameraPose,
    pub image: ImageInfo,
    pub render_time_seconds: f64,
    pub startup_marks: Option<serde_json::Value>,
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
fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderParams {
    pub map: String,
    pub camera: CameraPose,
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

    #[tool(description = "Render a headless base-map screenshot without opening or changing any visible window. Supply map and CS2 camera; size defaults 1138×640, max 4096 per side. Material time defaults zero. Reproduce a map-only frame_capture by copying its camera, image dimensions and render_time_seconds. Selected throws/effects are not included. Returns native PNG plus provenance.", output_schema = output_schema::<RenderResponse>(), annotations(read_only_hint = true, open_world_hint = false))]
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
            ),
        )
        .await
        {
            Ok(rendered) => {
                let metadata = RenderResponse {
                    summary: format!(
                        "Rendered {} headlessly at {}×{} pixels; visible window unchanged.",
                        rendered.map, rendered.width, rendered.height
                    ),
                    image: image_info(rendered.width, rendered.height, false, &rendered.png_base64),
                    map: rendered.map,
                    camera: rendered.camera,
                    render_time_seconds: rendered.render_time_seconds,
                    startup_marks: rendered.startup_marks,
                };
                with_image(metadata, rendered.png_base64)
            }
            Err(error) => self.fail(error).await,
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
    async fn protocol_discovers_nine_tools_and_validates_status_and_failure_schemas() {
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
                "camera_set",
                "diagnostics_get",
                "frame_capture",
                "map_open",
                "maps_list",
                "view_render"
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

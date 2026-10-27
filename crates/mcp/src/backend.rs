use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as PendingMutex};
use std::time::Duration;

use base64::Engine as _;
use nades_control::{
    AppCommand, AppMessage, AppState, CameraPose, ControlError, ControlOutput, ControlRequest,
    MAX_PNG_BYTES, PROTOCOL_VERSION, validate_map_id,
};
use nades_pack::Pack;
use schemars::JsonSchema;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, Semaphore, oneshot, watch};
use uuid::Uuid;

use crate::Config;

type Reply = Result<ControlOutput, ControlError>;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Snapshot {
    pub lifecycle: String,
    pub instance_id: Option<String>,
    pub requested_map: Option<String>,
    pub pid: Option<u32>,
    pub state: Option<AppState>,
    pub last_error: Option<ControlError>,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RenderedView {
    pub png_base64: String,
    pub map: String,
    pub camera: CameraPose,
    pub width: u32,
    pub height: u32,
    pub render_time_seconds: f64,
    pub startup_marks: Option<serde_json::Value>,
}

struct Live {
    id: String,
    launch_map: String,
    pid: u32,
    child: Mutex<Child>,
    input: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<PendingMutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    next_request: AtomicU64,
    state: Mutex<Option<AppState>>,
    confirmed_ready: AtomicBool,
}

struct Session {
    lifecycle: &'static str,
    live: Option<Arc<Live>>,
    last_instance: Option<String>,
    last_error: Option<ControlError>,
    log_path: Option<PathBuf>,
}

struct Inner {
    config: Config,
    session: Mutex<Session>,
    mutations: Arc<Mutex<()>>,
    rendering: Arc<Semaphore>,
    videos: crate::video::VideoJobs,
    detached: AtomicBool,
    shutdown: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct Backend(Arc<Inner>);

impl Backend {
    pub fn new(config: Config) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self(Arc::new(Inner {
            config,
            session: Mutex::new(Session {
                lifecycle: "stopped",
                live: None,
                last_instance: None,
                last_error: None,
                log_path: None,
            }),
            mutations: Arc::new(Mutex::new(())),
            rendering: Arc::new(Semaphore::new(1)),
            videos: crate::video::VideoJobs::default(),
            detached: AtomicBool::new(false),
            shutdown,
        }))
    }

    async fn mutation(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, ControlError> {
        tokio::time::timeout(
            Duration::from_secs(5),
            self.0.mutations.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            error(
                "busy",
                "Another application operation is still running. Read app_status before retrying.",
                true,
            )
        })
    }

    fn preflight(&self, map: &str) -> Result<PathBuf, ControlError> {
        validate_map_id(map)?;
        let path = self.0.config.packs_dir.join(format!("{map}.pack"));
        if !path.exists() {
            return Err(error(
                "pack_missing",
                format!("No local pack for {map}. Run `just pack {map}` first."),
                false,
            ));
        }
        let pack = Pack::open(&path).map_err(|e| error("pack_invalid", e.to_string(), false))?;
        if pack.contents().source.map != map {
            return Err(error(
                "pack_invalid",
                "The pack's map ID does not match its filename.",
                false,
            ));
        }
        pack.scene()
            .map_err(|e| error("pack_invalid", e.to_string(), false))?;
        Ok(path)
    }

    pub async fn start(&self, map: &str) -> Result<AppState, ControlError> {
        let mutation = self.mutation().await?;
        let this = self.clone();
        let map = map.to_owned();
        let (cancel, cancellation) = watch::channel(false);
        let mut guard = StartCancellation(Some(cancel));
        let result = tokio::spawn(async move {
            let _mutation = mutation;
            this.start_locked(&map, Some(cancellation)).await
        })
        .await
        .map_err(join_error)?;
        guard.0.take();
        result
    }

    async fn start_locked(
        &self,
        map: &str,
        mut cancellation: Option<watch::Receiver<bool>>,
    ) -> Result<AppState, ControlError> {
        if self.0.detached.load(Ordering::Acquire) {
            return Err(error(
                "disconnected",
                "The MCP session has disconnected.",
                false,
            ));
        }
        if start_cancelled(&cancellation) {
            return Err(error(
                "cancelled",
                "App startup was cancelled before launch.",
                false,
            ));
        }
        self.refresh_exit().await;
        let existing = self.0.session.lock().await.live.clone();
        if let Some(live) = existing {
            let state = state_output(live.request(AppCommand::GetState, 5).await?)?;
            if state.map == map {
                return Ok(state);
            }
            return Err(error(
                "already_running",
                "An app is already running on another map. Use map_open with its instance_id.",
                false,
            ));
        }
        let path = self.preflight(map)?;
        std::fs::create_dir_all(&self.0.config.runs_dir).map_err(io_error)?;
        let id = Uuid::new_v4().to_string();
        let log_path = self.0.config.runs_dir.join(format!("app-{id}.log"));
        let log = std::fs::File::create(&log_path).map_err(io_error)?;
        {
            let mut session = self.0.session.lock().await;
            if self.0.detached.load(Ordering::Acquire) || start_cancelled(&cancellation) {
                return Err(error(
                    "disconnected",
                    "App launch was abandoned after the MCP session disconnected or cancelled startup.",
                    false,
                ));
            }
            session.lifecycle = "starting";
            session.last_error = None;
            session.log_path = Some(log_path);
        }
        let spawned = Command::new(&self.0.config.sandbox_binary)
            .args(["--control-stdio", "--instance-id", &id, "--map", map])
            .env("NADES_PACK", &path)
            .env(
                "NADES_SANDBOX_STATE",
                self.0.config.runs_dir.join(format!("app-{id}-state.json")),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .kill_on_drop(false)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                let failure = error(
                    "launch_failed",
                    format!("Launching {}: {e}", self.0.config.sandbox_binary.display()),
                    false,
                );
                self.failed(failure.clone()).await;
                return Err(failure);
            }
        };
        let output = child.stdout.take().expect("piped child stdout");
        let input = child.stdin.take();
        let live = Arc::new(Live {
            id,
            launch_map: map.to_owned(),
            pid: child.id().expect("running child PID"),
            child: Mutex::new(child),
            input: Arc::new(Mutex::new(input)),
            pending: Arc::new(PendingMutex::new(HashMap::new())),
            next_request: AtomicU64::new(1),
            state: Mutex::new(None),
            confirmed_ready: AtomicBool::new(false),
        });
        let published = {
            let mut session = self.0.session.lock().await;
            if self.0.detached.load(Ordering::Acquire) || start_cancelled(&cancellation) {
                false
            } else {
                session.live = Some(live.clone());
                true
            }
        };
        let (ready_tx, ready_rx) = oneshot::channel();
        let reader_live = live.clone();
        tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            let mut lines = BufReader::new(output).lines();
            let failure = loop {
                match lines.next_line().await {
                    Ok(Some(line)) => match serde_json::from_str::<AppMessage>(&line) {
                        Ok(AppMessage::Ready {
                            protocol_version,
                            state,
                        }) => {
                            if protocol_version != PROTOCOL_VERSION
                                || state.instance_id != reader_live.id
                            {
                                break error(
                                    "protocol_mismatch",
                                    "The app uses an incompatible control protocol or instance ID.",
                                    false,
                                );
                            }
                            reader_live.confirmed_ready.store(true, Ordering::Release);
                            *reader_live.state.lock().await = Some(state.clone());
                            if let Some(sender) = ready.take() {
                                let _ = sender.send(Ok(state));
                            }
                        }
                        Ok(AppMessage::Detached { ready }) => {
                            if ready {
                                reader_live.confirmed_ready.store(true, Ordering::Release);
                            }
                        }
                        Ok(AppMessage::Reply { id, result }) => {
                            if let Ok(
                                ControlOutput::State { state }
                                | ControlOutput::Capture { state, .. },
                            ) = &result
                            {
                                *reader_live.state.lock().await = Some(state.clone());
                            }
                            if let Some(sender) = reader_live
                                .pending
                                .lock()
                                .expect("pending responses")
                                .remove(&id)
                            {
                                let _ = sender.send(result);
                            }
                        }
                        Err(e) => {
                            break error(
                                "protocol_error",
                                format!("Invalid app control response: {e}"),
                                false,
                            );
                        }
                    },
                    Ok(None) => {
                        break error(
                            "app_exited",
                            "The app's control connection closed. Inspect diagnostics_get for details.",
                            true,
                        );
                    }
                    Err(e) => break error("connection_failed", e.to_string(), true),
                }
            };
            if let Some(sender) = ready.take() {
                let _ = sender.send(Err(failure.clone()));
            }
            for (_, sender) in reader_live
                .pending
                .lock()
                .expect("pending responses")
                .drain()
            {
                let _ = sender.send(Err(failure.clone()));
            }
        });
        if !published {
            self.release_after_eof(&live).await;
            return Err(error(
                "disconnected",
                "App ownership was abandoned during launch; the child decides whether to exit or hand off its ready window.",
                false,
            ));
        }
        let ready = tokio::select! {
            ready = tokio::time::timeout(Duration::from_secs(60), ready_rx) => ready,
            _ = wait_start_cancellation(&mut cancellation) => {
                self.release_after_eof(&live).await;
                return Err(error("cancelled", "App startup was cancelled. Incomplete startup exits; an already-ready window is handed to the user.", false));
            }
        };
        match ready {
            Ok(Ok(Ok(state))) => {
                let mut session = self.0.session.lock().await;
                if self.0.detached.load(Ordering::Acquire) || start_cancelled(&cancellation) {
                    drop(session);
                    self.release_after_eof(&live).await;
                    return Err(error(
                        "disconnected",
                        "The ready window was handed to the user after MCP disconnected.",
                        false,
                    ));
                }
                session.lifecycle = "ready";
                Ok(state)
            }
            result => {
                if self.0.detached.load(Ordering::Acquire) || start_cancelled(&cancellation) {
                    self.release_after_eof(&live).await;
                    return Err(error(
                        "disconnected",
                        "The control session closed during startup. The child owns its readiness and handoff decision.",
                        false,
                    ));
                }
                let failure = match result {
                    Ok(Ok(Err(failure))) => failure,
                    _ => error(
                        "startup_timeout",
                        "The app did not present a visible frame within 60 seconds.",
                        true,
                    ),
                };
                let mut child = live.child.lock().await;
                let session = self.0.session.lock().await;
                if self.0.detached.load(Ordering::Acquire) {
                    drop(session);
                    drop(child);
                    self.release_after_eof(&live).await;
                    return Err(error(
                        "disconnected",
                        "MCP disconnected; startup timeout cannot terminate a handed-off window.",
                        false,
                    ));
                }
                let _ = child.start_kill();
                drop(session);
                let _ = child.wait().await;
                drop(child);
                self.failed(failure.clone()).await;
                Err(failure)
            }
        }
    }

    async fn failed(&self, failure: ControlError) {
        let mut session = self.0.session.lock().await;
        session.last_instance = session.live.as_ref().map(|live| live.id.clone());
        session.live = None;
        session.lifecycle = "failed";
        session.last_error = Some(failure);
    }

    async fn refresh_exit(&self) {
        let live = self.0.session.lock().await.live.clone();
        if let Some(live) = live {
            let Ok(mut child) = live.child.try_lock() else {
                return;
            };
            let exited = child.try_wait();
            if let Ok(Some(status)) = exited {
                let mut session = self.0.session.lock().await;
                if session
                    .live
                    .as_ref()
                    .is_some_and(|current| current.id == live.id)
                {
                    session.last_instance = Some(live.id.clone());
                    session.live = None;
                    session.lifecycle = if status.success() {
                        "stopped"
                    } else {
                        "failed"
                    };
                    session.last_error = if status.success() {
                        None
                    } else {
                        Some(error(
                            "app_exited",
                            format!("App exited with {status}."),
                            true,
                        ))
                    };
                }
            }
        }
    }

    pub async fn status(&self) -> Snapshot {
        self.refresh_exit().await;
        let (lifecycle, live) = {
            let session = self.0.session.lock().await;
            (session.lifecycle, session.live.clone())
        };
        let mut query_error = None;
        if lifecycle == "ready"
            && let Some(live) = &live
            && let Err(e) = live.request(AppCommand::GetState, 5).await
        {
            query_error = Some(e);
        }
        let session = self.0.session.lock().await;
        let state = if query_error.is_none()
            && let Some(current) = &session.live
        {
            current.state.lock().await.clone()
        } else {
            None
        };
        Snapshot {
            lifecycle: if query_error.is_some() {
                "failed".into()
            } else {
                session.lifecycle.into()
            },
            instance_id: session.live.as_ref().map(|live| live.id.clone()),
            requested_map: if session.lifecycle == "starting" {
                session.live.as_ref().map(|live| live.launch_map.clone())
            } else {
                state.as_ref().map(|state| state.map.clone())
            },
            pid: session.live.as_ref().map(|live| live.pid),
            state,
            last_error: query_error.or_else(|| session.last_error.clone()),
            log_path: session
                .log_path
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    }

    async fn target(&self, instance_id: &str) -> Result<Arc<Live>, ControlError> {
        self.refresh_exit().await;
        let session = self.0.session.lock().await;
        match &session.live {
            Some(live) if live.id != instance_id => Err(error(
                "stale_instance",
                "This instance was replaced. Read app_status and use its current instance_id.",
                false,
            )),
            Some(live) if session.lifecycle == "ready" => Ok(live.clone()),
            Some(_) => Err(error(
                "not_ready",
                "The app is not ready. Read app_status before retrying.",
                true,
            )),
            None => Err(error(
                "app_not_running",
                "No managed app is running. Call app_start first.",
                false,
            )),
        }
    }

    pub async fn stop(&self, instance_id: &str) -> Result<Snapshot, ControlError> {
        let mutation = self.mutation().await?;
        let this = self.clone();
        let instance_id = instance_id.to_owned();
        tokio::spawn(async move {
            let _mutation = mutation;
            this.stop_locked(&instance_id).await?;
            Ok(this.status().await)
        })
        .await
        .map_err(join_error)?
    }

    async fn stop_locked(&self, instance_id: &str) -> Result<(), ControlError> {
        self.refresh_exit().await;
        let live = {
            let session = self.0.session.lock().await;
            match &session.live {
                Some(live) if live.id == instance_id => live.clone(),
                Some(_) => {
                    return Err(error(
                        "stale_instance",
                        "The requested instance is not the managed window.",
                        false,
                    ));
                }
                None if session.last_instance.as_deref() == Some(instance_id) => return Ok(()),
                None => {
                    return Err(error(
                        "app_not_running",
                        "No matching managed app is running.",
                        false,
                    ));
                }
            }
        };
        self.0.session.lock().await.lifecycle = "stopping";
        let graceful = async {
            let _ = live.request(AppCommand::Shutdown, 1).await;
            live.child.lock().await.wait().await.map_err(io_error)
        };
        let stop_error = match tokio::time::timeout(Duration::from_secs(5), graceful).await {
            Ok(result) => {
                let status = result?;
                (!status.success()).then(|| error(
                    "app_exit_failed",
                    format!("The app stopped, but graceful shutdown exited with {status}. Inspect diagnostics_get for the app log."),
                    false,
                ))
            }
            Err(_) => {
                live.child.lock().await.kill().await.map_err(io_error)?;
                Some(error(
                    "app_stop_forced",
                    "The app did not exit within the 5-second graceful shutdown deadline. Its owned process was terminated and reaped; inspect diagnostics_get for details.",
                    false,
                ))
            }
        };
        live.input.lock().await.take();
        let mut session = self.0.session.lock().await;
        session.live = None;
        session.last_instance = Some(instance_id.to_owned());
        session.lifecycle = "stopped";
        session.last_error = stop_error.clone();
        match stop_error {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }

    pub async fn open_map(
        &self,
        instance_id: &str,
        map: &str,
        camera: Option<CameraPose>,
    ) -> Result<(AppState, bool), ControlError> {
        let mutation = self.mutation().await?;
        let this = self.clone();
        let instance_id = instance_id.to_owned();
        let map = map.to_owned();
        let (cancel, cancellation) = watch::channel(false);
        let mut guard = StartCancellation(Some(cancel));
        let result = tokio::spawn(async move {
            let _mutation = mutation;
            if *cancellation.borrow() { return Err(error("cancelled", "Map change was cancelled before application.", false)); }
            let camera = camera.map(CameraPose::validated).transpose()?;
            this.preflight(&map)?;
            let live = this.target(&instance_id).await?;
            let mut waiting = Some(cancellation.clone());
            let current = tokio::select! {
                biased;
                _ = wait_start_cancellation(&mut waiting) => return Err(error("cancelled", "Map change was cancelled before application.", false)),
                output = live.request(AppCommand::GetState, 5) => state_output(output?)?,
            };
            if *cancellation.borrow() { return Err(error("cancelled", "Map change was cancelled before application.", false)); }
            let restarted = current.map != map;
            let state = if restarted {
                this.stop_locked(&instance_id).await?;
                this.start_locked(&map, Some(cancellation.clone())).await?
            } else {
                current
            };
            if *cancellation.borrow() { return Err(error("cancelled", "Map startup or camera application was cancelled. Read app_status for the actual outcome.", false)); }
            let state = if let Some(camera) = camera {
                let target = this.target(&state.instance_id).await?;
                let mut cancel = Some(cancellation.clone());
                let output = tokio::select! {
                    result = target.request(AppCommand::SetCamera { camera, expected_revision: None }, 15) => result?,
                    _ = wait_start_cancellation(&mut cancel) => return Err(error("cancelled", "Map camera application was cancelled. Read app_status for the actual outcome.", false)),
                };
                state_output(output)?
            } else {
                state
            };
            Ok((state, restarted))
        })
        .await
        .map_err(join_error)?;
        guard.0.take();
        result
    }

    pub async fn set_camera(
        &self,
        instance_id: &str,
        camera: CameraPose,
        expected_revision: Option<u64>,
    ) -> Result<AppState, ControlError> {
        let _mutation = self.mutation().await?;
        let camera = camera.validated()?;
        let live = self.target(instance_id).await?;
        state_output(
            live.request(
                AppCommand::SetCamera {
                    camera,
                    expected_revision,
                },
                15,
            )
            .await?,
        )
    }

    pub async fn capture(
        &self,
        instance_id: &str,
        include_ui: bool,
        expected_revision: Option<u64>,
    ) -> Result<(AppState, String, u32, u32), ControlError> {
        let _mutation = self.mutation().await?;
        let live = self.target(instance_id).await?;
        match live
            .request(
                AppCommand::Capture {
                    include_ui,
                    expected_revision,
                },
                15,
            )
            .await?
        {
            ControlOutput::Capture {
                state,
                png_base64,
                width,
                height,
                ..
            } => {
                if png_base64.len() > MAX_PNG_BYTES.div_ceil(3) * 4 {
                    return Err(error(
                        "image_too_large",
                        "The captured PNG exceeds the 16 MiB limit.",
                        false,
                    ));
                }
                Ok((state, png_base64, width, height))
            }
            _ => Err(error(
                "protocol_error",
                "The app did not return a capture.",
                false,
            )),
        }
    }

    pub async fn render(
        &self,
        map: &str,
        camera: CameraPose,
        width: u32,
        height: u32,
        render_time_seconds: f64,
    ) -> Result<RenderedView, ControlError> {
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || !render_time_seconds.is_finite()
            || render_time_seconds < 0.0
        {
            return Err(error(
                "invalid_render",
                "Dimensions must be 1–4096 pixels; render_time_seconds must be finite and nonnegative.",
                false,
            ));
        }
        let camera = camera.validated()?;
        let mut shutdown = self.0.shutdown.subscribe();
        if *shutdown.borrow() {
            return Err(error(
                "disconnected",
                "The MCP session has disconnected.",
                false,
            ));
        }
        let _permit = self.0.rendering.try_acquire().map_err(|e| {
            if matches!(e, tokio::sync::TryAcquireError::Closed) {
                error("disconnected", "The MCP session has disconnected.", false)
            } else {
                error(
                    "busy",
                    "Another headless render is running. Retry after it completes.",
                    true,
                )
            }
        })?;
        let path = self.preflight(map)?;
        std::fs::create_dir_all(&self.0.config.runs_dir).map_err(io_error)?;
        let temp = tempfile::Builder::new()
            .prefix("render-")
            .tempdir_in(&self.0.config.runs_dir)
            .map_err(io_error)?;
        let out = temp.path().join("view.png");
        let log = std::fs::File::create(temp.path().join("stderr.log")).map_err(io_error)?;
        let position = camera.position.map(|v| v.to_string()).join(",");
        let angles = format!("{},{}", camera.pitch_degrees, camera.yaw_degrees);
        let mut command = Command::new(&self.0.config.sandbox_binary);
        command
            .arg("--render")
            .arg(&out)
            .args(["--map", map, "--setpos", &position, "--setang", &angles])
            .args([
                "--eye",
                &camera.eye_height_units.to_string(),
                "--fov",
                &camera.vertical_fov_degrees.to_string(),
                "--render-time",
                &render_time_seconds.to_string(),
                "--size",
                &format!("{width}x{height}"),
            ])
            .env("NADES_PACK", path)
            .env("NADES_SANDBOX_STATE", temp.path().join("state.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        let output = command.output();
        let output = tokio::select! {
            output = tokio::time::timeout(Duration::from_secs(60), output) => output.map_err(|_| error("render_timeout", "Offscreen rendering exceeded 60 seconds.", true))?.map_err(io_error)?,
            _ = shutdown.changed() => return Err(error("disconnected", "Offscreen rendering cancelled when MCP disconnected.", false)),
        };
        if !output.status.success() {
            let detail =
                std::fs::read_to_string(temp.path().join("stderr.log")).unwrap_or_default();
            return Err(error(
                "render_failed",
                format!(
                    "Offscreen rendering exited with {}: {}",
                    output.status,
                    tail_text(&detail, 10).join("\n")
                ),
                true,
            ));
        }
        let size = std::fs::metadata(&out).map_err(io_error)?.len();
        if size > MAX_PNG_BYTES as u64 {
            return Err(error(
                "image_too_large",
                "The rendered PNG exceeds the 16 MiB limit.",
                false,
            ));
        }
        let bytes = std::fs::read(&out).map_err(io_error)?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(error(
                "render_failed",
                "Renderer output is not a PNG.",
                false,
            ));
        }
        let startup_marks = String::from_utf8_lossy(&output.stdout)
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line).ok());
        Ok(RenderedView {
            png_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            map: map.to_owned(),
            camera,
            width,
            height,
            render_time_seconds,
            startup_marks,
        })
    }

    pub async fn video_start(
        &self,
        request: nades_control::video::VideoRequest,
    ) -> Result<crate::video::VideoStatus, ControlError> {
        let request = request.validated()?;
        if self.0.detached.load(Ordering::Acquire) {
            return Err(error("disconnected", "MCP session is disconnected.", false));
        }
        let pack = self.preflight(&request.map)?;
        let permit = self
            .0
            .rendering
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("busy", "Another headless render is running.", true))?;
        self.0
            .videos
            .start(
                request,
                self.0.config.clone(),
                pack,
                permit,
                self.0.shutdown.subscribe(),
            )
            .await
    }

    pub async fn video_status(&self, id: &str) -> Result<crate::video::VideoStatus, ControlError> {
        self.0.videos.status(id).await
    }

    pub async fn video_cancel(&self, id: &str) -> Result<crate::video::VideoStatus, ControlError> {
        self.0.videos.cancel(id).await
    }

    pub async fn logs(&self, limit: usize) -> Vec<String> {
        let path = self.0.session.lock().await.log_path.clone();
        path.and_then(|path| read_log_tail(&path).ok())
            .map(|text| tail_text(&text, limit.min(200)))
            .unwrap_or_default()
    }

    pub async fn detach(&self) {
        let live = {
            let session = self.0.session.lock().await;
            self.0.detached.store(true, Ordering::Release);
            self.0.shutdown.send_replace(true);
            self.0.rendering.close();
            session.live.clone()
        };
        if let Some(live) = live {
            self.release_after_eof(&live).await;
        }
        self.0.videos.wait_shutdown().await;
    }

    async fn release_after_eof(&self, live: &Live) {
        live.input.lock().await.take();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if live.confirmed_ready.load(Ordering::Acquire) {
                    return;
                }
                if live.child.lock().await.try_wait().ok().flatten().is_some() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if !live.confirmed_ready.load(Ordering::Acquire) {
            let mut child = live.child.lock().await;
            if !live.confirmed_ready.load(Ordering::Acquire)
                && child.try_wait().ok().flatten().is_none()
            {
                let _ = child.kill().await;
            }
        }
        let mut session = self.0.session.lock().await;
        if session
            .live
            .as_ref()
            .is_none_or(|current| current.id == live.id)
        {
            session.last_instance = Some(live.id.clone());
            session.live = None;
            session.lifecycle = "stopped";
        }
    }
}

struct StartCancellation(Option<watch::Sender<bool>>);

impl Drop for StartCancellation {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            cancel.send_replace(true);
        }
    }
}

fn start_cancelled(cancellation: &Option<watch::Receiver<bool>>) -> bool {
    cancellation.as_ref().is_some_and(|cancel| *cancel.borrow())
}

async fn wait_start_cancellation(cancellation: &mut Option<watch::Receiver<bool>>) {
    if let Some(cancel) = cancellation {
        loop {
            if *cancel.borrow_and_update() {
                return;
            }
            if cancel.changed().await.is_err() {
                break;
            }
        }
    }
    std::future::pending::<()>().await;
}

impl Live {
    async fn request(&self, command: AppCommand, seconds: u64) -> Reply {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending responses")
            .insert(id, sender);
        let _registration = PendingRegistration {
            pending: self.pending.clone(),
            input: Some(self.input.clone()),
            id,
        };
        let deadline_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| (now.as_millis() as u64).saturating_add(seconds * 1000))
            .ok();
        let mut bytes = serde_json::to_vec(&ControlRequest {
            id,
            deadline_unix_ms,
            command,
        })
        .map_err(|e| error("protocol_error", e.to_string(), false))?;
        bytes.push(b'\n');
        let result = async {
            {
                let mut guard = self.input.lock().await;
                let input = guard.as_mut().ok_or_else(|| {
                    error(
                        "disconnected",
                        "The app control connection is closed.",
                        false,
                    )
                })?;
                input.write_all(&bytes).await.map_err(io_error)?;
                input.flush().await.map_err(io_error)?;
            }
            receiver.await.map_err(|_| {
                error(
                    "connection_failed",
                    "The app closed the request channel.",
                    true,
                )
            })?
        };
        tokio::time::timeout(Duration::from_secs(seconds), result).await.unwrap_or_else(|_| Err(error("timeout", "The app did not complete this request before its deadline. Inspect app_status; a mutation may already have applied.", true)))
    }
}

struct PendingRegistration {
    pending: Arc<PendingMutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    input: Option<Arc<Mutex<Option<ChildStdin>>>>,
    id: u64,
}

impl Drop for PendingRegistration {
    fn drop(&mut self) {
        let waiting = self
            .pending
            .lock()
            .expect("pending responses")
            .remove(&self.id)
            .is_some();
        if waiting
            && let Some(input) = self.input.clone()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let request_id = self.id;
            runtime.spawn(async move {
                let Ok(mut bytes) = serde_json::to_vec(&ControlRequest {
                    id: 0,
                    deadline_unix_ms: None,
                    command: AppCommand::Cancel { request_id },
                }) else {
                    return;
                };
                bytes.push(b'\n');
                if let Some(writer) = input.lock().await.as_mut() {
                    let _ = writer.write_all(&bytes).await;
                    let _ = writer.flush().await;
                }
            });
        }
    }
}

fn state_output(output: ControlOutput) -> Result<AppState, ControlError> {
    match output {
        ControlOutput::State { state } => Ok(state),
        _ => Err(error(
            "protocol_error",
            "The app did not return a state snapshot.",
            false,
        )),
    }
}

fn read_log_tail(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(if start > 0 {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default()
    } else {
        text
    })
}

fn tail_text(text: &str, limit: usize) -> Vec<String> {
    let mut lines: Vec<_> = text
        .lines()
        .rev()
        .take(limit)
        .map(|line| line.chars().take(4096).collect())
        .collect();
    lines.reverse();
    lines
}

fn error(code: &str, message: impl Into<String>, retryable: bool) -> ControlError {
    ControlError::new(code, message, retryable)
}
fn io_error(e: std::io::Error) -> ControlError {
    error("io_error", e.to_string(), true)
}
fn join_error(e: tokio::task::JoinError) -> ControlError {
    error("internal_error", e.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(root: &std::path::Path) -> Backend {
        Backend::new(Config {
            sandbox_binary: root.join("unused"),
            packs_dir: root.to_owned(),
            runs_dir: root.join("runs"),
        })
    }

    fn valid_pack(root: &std::path::Path) {
        let scene = nades_pack::scene::Scene::default().to_bytes();
        nades_pack::write(
            &root.join("de_cache.pack"),
            nades_pack::Source {
                map: "de_cache".into(),
                game_build: "test".into(),
                export_hash: "test".into(),
            },
            &[(nades_pack::SectionKind::Scene, &scene)],
        )
        .unwrap();
    }

    #[cfg(unix)]
    fn mock_child(root: &std::path::Path, ready_after_eof: bool) -> Backend {
        use std::os::unix::fs::PermissionsExt;
        valid_pack(root);
        let script = root.join("child.py");
        let state = AppState {
            instance_id: "replaced-by-child".into(),
            map: "de_cache".into(),
            camera: CameraPose {
                position: [0.0; 3],
                pitch_degrees: 0.0,
                yaw_degrees: 0.0,
                eye_height_units: 64.0,
                vertical_fov_degrees: 73.739_8,
            },
            scene: Default::default(),
            viewport: nades_control::Viewport {
                width: 100,
                height: 100,
                scale_factor: 1.0,
                visible: true,
            },
            scene_viewport: nades_control::ViewportRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            frame_id: 1,
            revision: 1,
            presented_revision: 1,
            render_time_seconds: 0.0,
            gpu: "mock IPC boundary, not a GPU".into(),
            draws: 0,
            triangles: 0,
            startup_marks: vec![],
            pre_main_ms: None,
            metrics: Default::default(),
        };
        std::fs::write(
            root.join("ready.json"),
            serde_json::to_vec(&AppMessage::Ready {
                protocol_version: PROTOCOL_VERSION,
                state,
            })
            .unwrap(),
        )
        .unwrap();
        let code = format!(
            r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(__file__).parent
(root / "pid").write_text(str(os.getpid()))
for line in sys.stdin:
    pass
(root / "eof").write_text("received")
print(json.dumps({{"type": "detached", "ready": {ready_after_eof}}}), flush=True)
if {ready_after_eof}:
    time.sleep(5.2)
    ready = json.loads((root / "ready.json").read_text())
    ready["state"]["instance_id"] = sys.argv[sys.argv.index("--instance-id") + 1]
    print(json.dumps(ready), flush=True)
    time.sleep(60)
"#,
            ready_after_eof = if ready_after_eof { "True" } else { "False" }
        );
        std::fs::write(&script, code).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        Backend::new(Config {
            sandbox_binary: script,
            packs_dir: root.to_owned(),
            runs_dir: root.join("runs"),
        })
    }

    #[cfg(unix)]
    async fn wait_live(backend: &Backend) -> Arc<Live> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(live) = backend.0.session.lock().await.live.clone() {
                    return live;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mock child ownership published")
    }

    #[cfg(unix)]
    fn responding_child(root: &std::path::Path, shutdown: &str) -> Backend {
        let backend = mock_child(root, false);
        let code = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys, time
root = pathlib.Path(__file__).parent
ready = json.loads((root / "ready.json").read_text())
ready["state"]["instance_id"] = sys.argv[sys.argv.index("--instance-id") + 1]
print(json.dumps(ready), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    method = request["command"]["method"]
    if method == "cancel":
        continue
    if method == "shutdown":
        print(json.dumps({{"type":"reply","id":request["id"],"result":{{"Ok":{{"kind":"stopped"}}}}}}), flush=True)
        {shutdown}
    print(json.dumps({{"type":"reply","id":request["id"],"result":{{"Ok":{{"kind":"state","state":ready["state"]}}}}}}), flush=True)
"#
        );
        std::fs::write(&backend.0.config.sandbox_binary, code).unwrap();
        backend
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopped_reply_does_not_mask_a_nonzero_shutdown_exit() {
        let root = tempfile::tempdir().unwrap();
        let backend = responding_child(root.path(), "sys.exit(7)");
        let state = backend.start("de_cache").await.unwrap();
        let live = backend.0.session.lock().await.live.clone().unwrap();
        let failure = backend.stop(&state.instance_id).await.unwrap_err();
        assert_eq!(failure.code, "app_exit_failed");
        assert_eq!(
            live.child.lock().await.try_wait().unwrap().unwrap().code(),
            Some(7)
        );
        let status = backend.status().await;
        assert_eq!(status.lifecycle, "stopped");
        assert_eq!(status.pid, None);
        assert_eq!(status.last_error.as_ref().unwrap().code, "app_exit_failed");
        let repeated = backend.stop(&state.instance_id).await.unwrap();
        assert_eq!(repeated.lifecycle, "stopped");
        assert_eq!(repeated.last_error.unwrap().code, "app_exit_failed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forced_shutdown_is_reported_and_does_not_touch_another_instance() {
        let root = tempfile::tempdir().unwrap();
        let other_root = tempfile::tempdir().unwrap();
        let backend = responding_child(root.path(), "time.sleep(60)");
        let other = responding_child(other_root.path(), "sys.exit(0)");
        let state = backend.start("de_cache").await.unwrap();
        let other_state = other.start("de_cache").await.unwrap();
        let live = backend.0.session.lock().await.live.clone().unwrap();
        let failure = backend.stop(&state.instance_id).await.unwrap_err();
        assert_eq!(failure.code, "app_stop_forced");
        assert!(live.child.lock().await.try_wait().unwrap().is_some());
        let stopped = backend.status().await;
        assert_eq!(stopped.lifecycle, "stopped");
        assert_eq!(stopped.last_error.unwrap().code, "app_stop_forced");
        assert_eq!(other.status().await.lifecycle, "ready");
        other.stop(&other_state.instance_id).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reuse_and_map_replacement_follow_the_ui_map_not_the_launch_map() {
        let root = tempfile::tempdir().unwrap();
        let backend = mock_child(root.path(), false);
        let scene = nades_pack::scene::Scene::default().to_bytes();
        nades_pack::write(
            &root.path().join("de_mirage.pack"),
            nades_pack::Source {
                map: "de_mirage".into(),
                game_build: "test".into(),
                export_hash: "test".into(),
            },
            &[(nades_pack::SectionKind::Scene, &scene)],
        )
        .unwrap();
        let script = r#"#!/usr/bin/env python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
ready = json.loads((root / "ready.json").read_text())
state = ready["state"]
state["instance_id"] = sys.argv[sys.argv.index("--instance-id") + 1]
state["map"] = sys.argv[sys.argv.index("--map") + 1]
print(json.dumps(ready), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    method = request["command"]["method"]
    if method == "cancel":
        continue
    if method == "shutdown":
        print(json.dumps({"type":"reply","id":request["id"],"result":{"Ok":{"kind":"stopped"}}}), flush=True)
        break
    change = root / "ui-map"
    if change.exists():
        state["map"] = change.read_text()
        change.unlink()
        state["revision"] += 1
        state["presented_revision"] = state["revision"]
    print(json.dumps({"type":"reply","id":request["id"],"result":{"Ok":{"kind":"state","state":state}}}), flush=True)
"#;
        std::fs::write(&backend.0.config.sandbox_binary, script).unwrap();
        let first = backend.start("de_cache").await.unwrap();
        std::fs::write(root.path().join("ui-map"), "de_mirage").unwrap();
        assert_eq!(
            backend.start("de_cache").await.unwrap_err().code,
            "already_running"
        );
        let reused = backend.start("de_mirage").await.unwrap();
        assert_eq!(reused.instance_id, first.instance_id);
        assert_eq!(reused.map, "de_mirage");
        assert_eq!(
            backend.status().await.requested_map.as_deref(),
            Some("de_mirage")
        );
        let (same, restarted) = backend
            .open_map(&first.instance_id, "de_mirage", None)
            .await
            .unwrap();
        assert!(!restarted);
        assert_eq!(same.instance_id, first.instance_id);
        assert_eq!(same.map, "de_mirage");
        let (replacement, restarted) = backend
            .open_map(&first.instance_id, "de_cache", None)
            .await
            .unwrap();
        assert!(restarted);
        assert_ne!(replacement.instance_id, first.instance_id);
        assert_eq!(replacement.map, "de_cache");
        backend.stop(&replacement.instance_id).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_start_closes_incomplete_child_and_releases_ownership() {
        let root = tempfile::tempdir().unwrap();
        let backend = mock_child(root.path(), false);
        let start_backend = backend.clone();
        let startup = tokio::spawn(async move { start_backend.start("de_cache").await });
        let live = wait_live(&backend).await;
        startup.abort();
        let _ = startup.await;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if backend.status().await.lifecycle == "stopped" {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled startup cleaned up");
        assert_eq!(
            std::fs::read_to_string(root.path().join("eof")).unwrap(),
            "received"
        );
        assert!(
            live.child
                .lock()
                .await
                .try_wait()
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(backend.0.session.lock().await.live.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn disconnect_does_not_kill_a_ready_child_with_a_late_acknowledgement() {
        let root = tempfile::tempdir().unwrap();
        let backend = mock_child(root.path(), true);
        let start_backend = backend.clone();
        let startup = tokio::spawn(async move { start_backend.start("de_cache").await });
        let live = wait_live(&backend).await;
        backend.detach().await;
        assert!(live.child.lock().await.try_wait().unwrap().is_none());
        let result = tokio::time::timeout(Duration::from_secs(10), startup)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().code, "disconnected");
        assert!(live.state.lock().await.is_some());
        assert!(live.child.lock().await.try_wait().unwrap().is_none());
        live.child.lock().await.kill().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn disconnect_while_start_waits_for_state_prevents_a_later_spawn() {
        let root = tempfile::tempdir().unwrap();
        let backend = mock_child(root.path(), false);
        let session = backend.0.session.lock().await;
        let start_backend = backend.clone();
        let startup = tokio::spawn(async move { start_backend.start("de_cache").await });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        let detach_backend = backend.clone();
        let detach = tokio::spawn(async move { detach_backend.detach().await });
        tokio::task::yield_now().await;
        drop(session);
        detach.await.unwrap();
        assert_eq!(startup.await.unwrap().unwrap_err().code, "disconnected");
        assert!(!root.path().join("pid").exists());
        assert!(backend.0.session.lock().await.live.is_none());
    }

    #[tokio::test]
    async fn concurrent_headless_render_is_rejected_without_waiting_or_launching() {
        let root = tempfile::tempdir().unwrap();
        valid_pack(root.path());
        let backend = backend(root.path());
        let _permit = backend.0.rendering.acquire().await.unwrap();
        let camera = CameraPose {
            position: [0.0; 3],
            pitch_degrees: 0.0,
            yaw_degrees: 0.0,
            eye_height_units: 64.0,
            vertical_fov_degrees: 73.739_8,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            backend.render("de_cache", camera, 100, 100, 0.0),
        )
        .await
        .expect("busy is immediate");
        assert_eq!(result.unwrap_err().code, "busy");
        assert!(!root.path().join("runs").exists());
    }

    #[test]
    fn log_tail_preserves_order_and_bounds_records() {
        assert_eq!(
            tail_text("first\nsecond\nthird\n", 2),
            vec!["second", "third"]
        );
        assert_eq!(tail_text(&"x".repeat(9000), 1)[0].len(), 4096);
    }

    #[test]
    fn pack_preflight_checks_the_real_pack_and_its_identity() {
        let root = tempfile::tempdir().unwrap();
        let backend = backend(root.path());
        assert_eq!(
            backend.preflight("de_cache").unwrap_err().code,
            "pack_missing"
        );
        std::fs::write(root.path().join("de_cache.pack"), b"not a pack").unwrap();
        assert_eq!(
            backend.preflight("de_cache").unwrap_err().code,
            "pack_invalid"
        );
        let scene = nades_pack::scene::Scene::default().to_bytes();
        let source = nades_pack::Source {
            map: "de_other".into(),
            game_build: "test".into(),
            export_hash: "test".into(),
        };
        nades_pack::write(
            &root.path().join("de_cache.pack"),
            source.clone(),
            &[(nades_pack::SectionKind::Scene, &scene)],
        )
        .unwrap();
        assert_eq!(
            backend.preflight("de_cache").unwrap_err().message,
            "The pack's map ID does not match its filename."
        );
        nades_pack::write(
            &root.path().join("de_cache.pack"),
            nades_pack::Source {
                map: "de_cache".into(),
                ..source
            },
            &[(nades_pack::SectionKind::Scene, &scene)],
        )
        .unwrap();
        assert_eq!(
            backend.preflight("de_cache").unwrap(),
            root.path().join("de_cache.pack")
        );
    }

    #[tokio::test]
    async fn invalid_render_is_rejected_before_launching_or_creating_outputs() {
        let root = tempfile::tempdir().unwrap();
        let backend = backend(root.path());
        let camera = CameraPose {
            position: [0.0; 3],
            pitch_degrees: 0.0,
            yaw_degrees: 0.0,
            eye_height_units: 64.0,
            vertical_fov_degrees: 73.739_8,
        };
        let failure = backend
            .render("de_cache", camera, 0, 640, 0.0)
            .await
            .unwrap_err();
        assert_eq!(failure.code, "invalid_render");
        assert!(!root.path().join("runs").exists());
        assert_eq!(backend.status().await.lifecycle, "stopped");
    }

    #[test]
    fn dropped_request_registration_cannot_accumulate_pending_senders() {
        let pending = Arc::new(PendingMutex::new(HashMap::new()));
        let (sender, mut receiver) = oneshot::channel();
        pending.lock().unwrap().insert(7, sender);
        let registration = PendingRegistration {
            pending: pending.clone(),
            input: None,
            id: 7,
        };
        drop(registration);
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }
}

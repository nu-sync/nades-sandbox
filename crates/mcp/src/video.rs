use std::{collections::HashMap, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use nades_control::{ControlError, video::VideoRequest};
use schemars::JsonSchema;
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{Mutex, OwnedSemaphorePermit, watch},
};
use uuid::Uuid;

use crate::Config;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VideoStatus {
    pub job_id: String,
    pub state: String,
    pub frames_completed: u32,
    pub frames_total: u32,
    pub path: Option<String>,
    pub error: Option<String>,
    pub map: String,
    /// Throws in the scene.
    pub throws: usize,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration_seconds: f64,
    pub request: VideoRequest,
    /// What each throw of the scene did, once the renderer has simulated them.
    pub reports: Vec<nades_control::scene::ThrowReport>,
    /// The JSON beside the finished video: the scene, the stills and the reports.
    pub report_path: Option<String>,
}
struct Job {
    status: VideoStatus,
    cancel: watch::Sender<bool>,
}
#[derive(Clone, Default)]
pub struct VideoJobs {
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

impl VideoJobs {
    pub async fn start(
        &self,
        request: VideoRequest,
        output: Option<PathBuf>,
        config: Config,
        pack: PathBuf,
        permit: OwnedSemaphorePermit,
        shutdown: watch::Receiver<bool>,
    ) -> Result<VideoStatus, ControlError> {
        let id = Uuid::new_v4().to_string();
        let (cancel, cancelled) = watch::channel(false);
        let status = VideoStatus {
            job_id: id.clone(),
            state: "running".into(),
            frames_completed: 0,
            frames_total: request.frame_count().unwrap_or(0),
            path: None,
            error: None,
            map: request.scene.map.clone(),
            throws: request.scene.throws.len(),
            width: request.width,
            height: request.height,
            fps: request.fps,
            duration_seconds: f64::from(request.frame_count().unwrap_or(0)) / f64::from(request.fps),
            request: request.clone(),
            reports: Vec::new(),
            report_path: None,
        };
        self.jobs.lock().await.insert(
            id.clone(),
            Job {
                status: status.clone(),
                cancel,
            },
        );
        let jobs = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = jobs
                .run(&id, request, output, config, pack, cancelled, shutdown)
                .await;
            if let Some(job) = jobs.jobs.lock().await.get_mut(&id) {
                match result {
                    Ok(path) => {
                        job.status.state = "completed".into();
                        job.status.path = Some(path.to_string_lossy().into_owned());
                        // what happened, beside the video
                        let report = path.with_extension("json");
                        let what = serde_json::json!({"video": path, "stills": job.status.request.stills, "scene": job.status.request.scene, "reports": job.status.reports});
                        if std::fs::write(&report, serde_json::to_vec_pretty(&what).unwrap_or_default()).is_ok() {
                            job.status.report_path = Some(report.to_string_lossy().into_owned());
                        }
                    }
                    Err(e) if e == "cancelled" => {
                        job.status.state = "cancelled".into();
                    }
                    Err(e) => {
                        job.status.state = "failed".into();
                        job.status.error = Some(e);
                    }
                }
            }
        });
        Ok(status)
    }

    pub async fn wait_shutdown(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if self
                    .jobs
                    .lock()
                    .await
                    .values()
                    .all(|j| j.status.state != "running")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
    }

    pub async fn status(&self, id: &str) -> Result<VideoStatus, ControlError> {
        self.jobs
            .lock()
            .await
            .get(id)
            .map(|j| j.status.clone())
            .ok_or_else(|| {
                ControlError::new(
                    "unknown_video",
                    "No video job with this ID exists in this MCP session.",
                    false,
                )
            })
    }
    pub async fn cancel(&self, id: &str) -> Result<VideoStatus, ControlError> {
        let jobs = self.jobs.lock().await;
        let job = jobs.get(id).ok_or_else(|| {
            ControlError::new(
                "unknown_video",
                "No video job with this ID exists in this MCP session.",
                false,
            )
        })?;
        job.cancel.send_replace(true);
        Ok(job.status.clone())
    }
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        id: &str,
        request: VideoRequest,
        destination: Option<PathBuf>,
        config: Config,
        pack: PathBuf,
        mut cancel: watch::Receiver<bool>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&config.runs_dir).map_err(|e| e.to_string())?;
        let temp = tempfile::Builder::new()
            .prefix("video-")
            .tempdir_in(&config.runs_dir)
            .map_err(|e| e.to_string())?;
        let request_path = temp.path().join("request.json");
        std::fs::write(
            &request_path,
            serde_json::to_vec(&request).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let partial = temp.path().join("partial.mp4");
        let final_path = config.runs_dir.join(format!("video-{id}.mp4"));
        let stderr_path = temp.path().join("ffmpeg.log");
        let ffmpeg_log = std::fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
        let mut encoder = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
                &format!("{}x{}", request.width, request.height),
                "-framerate",
                &request.fps.to_string(),
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ])
            .arg(&partial)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(ffmpeg_log))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Cannot start ffmpeg; install ffmpeg with libx264: {e}"))?;
        let worker = Command::new(&config.sandbox_binary)
            .arg("--render-video")
            .arg(&request_path)
            .env("NADES_RAW_FRAMES", "1")
            .env("NADES_PACK", pack)
            .env("NADES_SANDBOX_PACKS", &config.packs_dir)
            .env("NADES_SANDBOX_STATE", temp.path().join("state.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut worker = match worker {
            Ok(worker) => worker,
            Err(e) => {
                let _ = encoder.kill().await;
                let _ = encoder.wait().await;
                return Err(format!("Cannot start renderer: {e}"));
            }
        };
        let mut output = worker.stdout.take().ok_or("Renderer stdout unavailable")?;
        let mut input = encoder.stdin.take().ok_or("Encoder stdin unavailable")?;
        let stderr = worker.stderr.take().ok_or("Renderer stderr unavailable")?;
        let jobs = self.jobs.clone();
        let id_for_progress = id.to_string();
        let progress = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tail = Vec::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                    && v.get("event").and_then(|x| x.as_str()) == Some("video_progress")
                {
                    if let Some(job) = jobs.lock().await.get_mut(&id_for_progress) {
                        job.status.frames_completed = v["frames"].as_u64().unwrap_or(0) as u32;
                        // a shot's length is known once the renderer has simulated its throws
                        job.status.frames_total = v["total"].as_u64().unwrap_or(0) as u32;
                        job.status.duration_seconds = f64::from(job.status.frames_total) / f64::from(job.status.fps);
                    }
                    continue;
                }
                if let Ok(report) = serde_json::from_str::<nades_control::scene::ThrowReport>(&line) {
                    if let Some(job) = jobs.lock().await.get_mut(&id_for_progress) {
                        job.status.reports.push(report);
                    }
                    continue;
                }
                tail.push(line);
                if tail.len() > 10 {
                    tail.remove(0);
                }
            }
            tail.join("\n")
        });
        let interrupted;
        let outcomes = {
            let process = async {
                let copy = tokio::io::copy(&mut output, &mut input).await;
                drop(input);
                drop(output);
                let worker_status = worker.wait().await;
                let encoder_status = encoder.wait().await;
                (copy, worker_status, encoder_status)
            };
            tokio::pin!(process);
            if *cancel.borrow() || *shutdown.borrow() {
                interrupted = Some("cancelled".to_string());
                None
            } else {
                tokio::select! {
                    outcome = &mut process => { interrupted = None; Some(outcome) },
                    _ = cancel.changed() => { interrupted = Some("cancelled".into()); None },
                    _ = shutdown.changed() => { interrupted = Some("cancelled".into()); None },
                    _ = tokio::time::sleep(Duration::from_secs(600)) => { interrupted = Some("Video render exceeded 10 minutes.".into()); None },
                }
            }
        };
        if let Some(message) = interrupted {
            let _ = worker.kill().await;
            let _ = encoder.kill().await;
            let _ = worker.wait().await;
            let _ = encoder.wait().await;
            let _ = progress.await;
            return Err(message);
        }
        let (copy, worker_status, encoder_status) =
            outcomes.expect("completed process has outcomes");
        let worker_log = progress.await.unwrap_or_default();
        let worker_status = worker_status.map_err(|e| e.to_string())?;
        let encoder_status = encoder_status.map_err(|e| e.to_string())?;
        let encoder_log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        if !encoder_status.success() && encoder_log.contains("Unknown encoder") {
            return Err(format!("FFmpeg needs libx264 support: {encoder_log}"));
        }
        if !worker_status.success() {
            return Err(format!("Renderer exited {worker_status}: {worker_log}"));
        }
        if !encoder_status.success() {
            return Err(format!("FFmpeg exited {encoder_status}: {encoder_log}"));
        }
        copy.map_err(|e| format!("Frame stream failed: {e}"))?;
        std::fs::rename(&partial, &final_path)
            .map_err(|e| format!("Publishing video failed: {e}"))?;
        match destination {
            Some(destination) => {
                std::fs::copy(&final_path, &destination).map_err(|e| format!("Copying the video to {} failed: {e}", destination.display()))?;
                let _ = std::fs::remove_file(&final_path);
                Ok(destination)
            }
            None => Ok(final_path),
        }
    }
}

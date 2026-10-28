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
    /// A lineup video's camera checked frame by frame against what the map draws: the chase and the pull-out.
    pub camera: Option<nades_control::video::CameraAudit>,
    /// The finished MP4's size, megabytes.
    pub megabytes: Option<f64>,
    /// Where the render's time went (S38): its wall clock, the renderer's setup and each frame's stages; once finished.
    pub timings: Option<nades_control::video::VideoTimings>,
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
            camera: None,
            megabytes: None,
            report_path: None,
            timings: None,
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
                        job.status.megabytes = std::fs::metadata(&path).ok().map(|m| m.len() as f64 / 1e6);
                        let what = serde_json::json!({"video": path, "stills": job.status.request.stills, "scene": job.status.request.scene, "reports": job.status.reports, "camera": job.status.camera, "megabytes": job.status.megabytes, "timings": job.status.timings});
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
        let launched = std::time::Instant::now();
        let reaped = children_cpu_seconds();
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
            Err(e) => return Err(format!("Cannot start renderer: {e}")),
        };
        let output = worker.stdout.take().ok_or("Renderer stdout unavailable")?;
        let stderr = worker.stderr.take().ok_or("Renderer stderr unavailable")?;
        let jobs = self.jobs.clone();
        let id_for_progress = id.to_string();
        let (planned, plan) = tokio::sync::oneshot::channel::<u64>();
        let progress = tokio::spawn(async move {
            let mut planned = Some(planned);
            let mut lines = BufReader::new(stderr).lines();
            let mut tail = Vec::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    match v.get("event").and_then(|x| x.as_str()) {
                        Some("video_plan") => {
                            if let Some(planned) = planned.take() {
                                let _ = planned.send(v["total"].as_u64().unwrap_or(0));
                            }
                            continue;
                        }
                        Some("video_timings") => {
                            if let (Ok(timings), Some(job)) = (serde_json::from_value::<nades_control::video::VideoTimings>(v.clone()), jobs.lock().await.get_mut(&id_for_progress)) {
                                job.status.timings = Some(timings);
                            }
                            continue;
                        }
                        Some("camera_audit") => {
                            if let (Ok(audit), Some(job)) = (serde_json::from_value::<nades_control::video::CameraAudit>(v.clone()), jobs.lock().await.get_mut(&id_for_progress)) {
                                job.status.camera = Some(audit);
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
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
        // the clip's length, before its first frame, sets the encoder's bitrate to keep the MP4 under its size
        let frames = tokio::select! {
            frames = plan => frames.ok(),
            _ = cancel.changed() => None,
            _ = shutdown.changed() => None,
            _ = tokio::time::sleep(Duration::from_secs(300)) => None,
        };
        let Some(frames) = frames.filter(|f| *f > 0) else {
            let _ = worker.kill().await;
            let status = worker.wait().await;
            let log = progress.await.unwrap_or_default();
            if *cancel.borrow() || *shutdown.borrow() {
                return Err("cancelled".into());
            }
            return Err(format!("Renderer exited {}: {log}", status.map_or_else(|e| e.to_string(), |s| s.to_string())));
        };
        let planned = launched.elapsed();
        // the renderer's frames go straight into the encoder: relayed through this process they cost a third more (S38)
        let frames_in: Stdio = output.try_into().map_err(|e| format!("Renderer stdout unavailable: {e}"))?;
        let seconds = frames as f64 / f64::from(request.fps);
        let kbits = bitrate_cap(request.max_megabytes.unwrap_or(nades_control::video::DEFAULT_MAX_MEGABYTES), seconds);
        let encoder = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-s",
                &format!("{}x{}", request.width, request.height),
                "-framerate",
                &request.fps.to_string(),
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                "libx264",
                // x264 `faster` at CRF 19: two thirds of `medium`'s CPU for the same look under the size's cap (S41)
                "-preset",
                "faster",
                "-crf",
                "19",
                "-maxrate",
                &format!("{kbits}k"),
                "-bufsize",
                &format!("{}k", kbits * 2),
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ])
            .arg(&partial)
            .stdin(frames_in)
            .stdout(Stdio::null())
            .stderr(Stdio::from(ffmpeg_log))
            .kill_on_drop(true)
            .spawn();
        let mut encoder = match encoder {
            Ok(encoder) => encoder,
            Err(e) => {
                let _ = worker.kill().await;
                let _ = worker.wait().await;
                return Err(format!("Cannot start ffmpeg; install ffmpeg with libx264: {e}"));
            }
        };
        let interrupted;
        let outcomes = {
            let process = async {
                let worker_status = worker.wait().await;
                let streamed = launched.elapsed();
                let encoder_status = encoder.wait().await;
                (worker_status, encoder_status, streamed, launched.elapsed())
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
        let (worker_status, encoder_status, streamed, encoded) =
            outcomes.expect("completed process has outcomes");
        let worker_log = progress.await.unwrap_or_default();
        if let Some(timings) = self.jobs.lock().await.get_mut(id).and_then(|job| job.status.timings.as_mut()) {
            timings.startup_seconds = planned.as_secs_f64();
            timings.frames_seconds = (streamed - planned).as_secs_f64();
            timings.encode_tail_seconds = (encoded - streamed).as_secs_f64();
            timings.total_seconds = encoded.as_secs_f64();
            // one headless job at a time: the children reaped meanwhile are its renderer and its encoder
            timings.encoder.cpu_seconds = match (reaped, children_cpu_seconds(), timings.renderer.cpu_seconds) {
                (Some(before), Some(after), Some(renderer)) => Some((after - before - renderer).max(0.0)),
                _ => None,
            };
        }
        let worker_status = worker_status.map_err(|e| e.to_string())?;
        let encoder_status = encoder_status.map_err(|e| e.to_string())?;
        let encoder_log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        if !encoder_status.success() && encoder_log.contains("Unknown encoder") {
            return Err(format!("FFmpeg needs libx264 support: {encoder_log}"));
        }
        // an encoder that fails closes the renderer's pipe under it: its failure is the cause
        if !encoder_status.success() {
            return Err(format!("FFmpeg exited {encoder_status}: {encoder_log}"));
        }
        if !worker_status.success() {
            return Err(format!("Renderer exited {worker_status}: {worker_log}"));
        }
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

/// The CPU time, user and system, of every child process this one has reaped.
#[cfg(unix)]
fn children_cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the rusage it is given and reports failure with -1
    let usage = (unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) } == 0).then(|| unsafe { usage.assume_init() })?;
    let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some(seconds(usage.ru_utime) + seconds(usage.ru_stime))
}

#[cfg(not(unix))]
fn children_cpu_seconds() -> Option<f64> {
    None
}

/// The bitrate, kbit/s, that keeps `seconds` of video under `megabytes` (8% left for the container and the encoder's
/// overshoot), at least 500.
fn bitrate_cap(megabytes: f64, seconds: f64) -> u64 {
    ((megabytes * 8000.0 * 0.92 / seconds.max(0.5)) as u64).max(500)
}

#[cfg(test)]
mod size {
    #[test]
    fn a_lineup_video_keeps_under_its_size() {
        // 25 MB over a 27.5 s lineup: about 6.7 Mbit/s
        assert_eq!(super::bitrate_cap(25.0, 27.5), 6690);
        assert_eq!(super::bitrate_cap(1.0, 60.0), 500);
    }
}

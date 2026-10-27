use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use nades_control::{AppState, CameraPose};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, CancelledNotificationParam,
    ClientRequest,
};
use rmcp::service::{PeerRequestOptions, RunningService, ServiceError};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::process::{Child, Command};

const TOOLS: [&str; 13] = [
    "app_start",
    "app_status",
    "app_stop",
    "camera_set",
    "diagnostics_get",
    "frame_capture",
    "lineups_list",
    "map_open",
    "maps_list",
    "video_cancel",
    "video_render",
    "video_status",
    "view_render",
];

#[derive(Clone)]
struct Config {
    root: PathBuf,
    server: PathBuf,
    sandbox: PathBuf,
    packs: PathBuf,
    output: PathBuf,
    keep_handoff: bool,
    database: PathBuf,
}

impl Config {
    fn parse() -> Result<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("crate parent")?
            .parent()
            .context("workspace parent")?
            .to_path_buf();
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let mut config = Self {
            server: root.join("target/release/nades-mcp"),
            sandbox: root.join("target/release/nades-sandbox"),
            packs: std::env::var_os("NADES_PACKS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                        .join(".cache/nades/packs")
                }),
            output: root.join(format!(".runs/mcp-{unique}")),
            keep_handoff: false,
            database: PathBuf::new(),
            root,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--keep-handoff" {
                config.keep_handoff = true;
                continue;
            }
            let value = args
                .next()
                .with_context(|| format!("{arg} requires a path"))?;
            match arg.as_str() {
                "--server" => config.server = value.into(),
                "--sandbox" => config.sandbox = value.into(),
                "--packs-dir" => config.packs = value.into(),
                "--output" => config.output = value.into(),
                _ => bail!(
                    "unknown option {arg}; supported: --server, --sandbox, --packs-dir, --output, --keep-handoff"
                ),
            }
        }
        Ok(config)
    }
}

struct Evidence {
    output: PathBuf,
    records: File,
    passed: Vec<String>,
    owned_pids: BTreeMap<u32, String>,
    calls: BTreeMap<String, (usize, usize)>,
    negative_cases: BTreeSet<String>,
    manual_handoff: Option<(u32, String)>,
    private_database: Option<PathBuf>,
}

impl Evidence {
    fn new(output: PathBuf) -> Result<Self> {
        fs::create_dir_all(output.parent().context("output parent")?)?;
        fs::create_dir(&output)
            .with_context(|| format!("create fresh evidence directory {}", output.display()))?;
        let records = File::create(output.join("tools.jsonl"))?;
        Ok(Self {
            output,
            records,
            passed: Vec::new(),
            owned_pids: BTreeMap::new(),
            calls: BTreeMap::new(),
            negative_cases: BTreeSet::new(),
            manual_handoff: None,
            private_database: None,
        })
    }

    fn check(&mut self, name: &str, condition: bool) -> Result<()> {
        ensure!(condition, "acceptance failed: {name}");
        self.passed.push(name.into());
        println!("PASS {name}");
        Ok(())
    }

    fn record(&mut self, tool: &str, arguments: &Value, result: &CallToolResult) -> Result<()> {
        let counts = self.calls.entry(tool.into()).or_default();
        if result.is_error == Some(true) {
            counts.1 += 1;
            self.negative_cases.insert(tool.into());
        } else {
            counts.0 += 1;
        }
        let data = json!({"tool": tool, "arguments": arguments, "is_error": result.is_error.unwrap_or(false), "structured_content": result.structured_content, "image_count": result.content.iter().filter(|c| c.as_image().is_some()).count()});
        serde_json::to_writer(&mut self.records, &data)?;
        self.records.write_all(b"\n")?;
        self.records.flush()?;
        if let Some(data) = result.structured_content.as_ref()
            && let Some(pid) = find_pid(data)
            && let Some(instance_id) = find_value(data, "instance_id").and_then(Value::as_str)
        {
            self.owned_pids.insert(pid, instance_id.into());
        }
        Ok(())
    }

    fn image(&self, result: &CallToolResult, name: &str) -> Result<DecodedPng> {
        let images: Vec<_> = result.content.iter().filter_map(|c| c.as_image()).collect();
        ensure!(
            images.len() == 1,
            "{name}: expected one native MCP image, received {}",
            images.len()
        );
        ensure!(
            images[0].mime_type == "image/png",
            "{name}: image is not image/png"
        );
        let bytes = base64::engine::general_purpose::STANDARD.decode(&images[0].data)?;
        fs::write(self.output.join(format!("{name}.png")), &bytes)?;
        decode_png(&bytes)
    }

    fn finish(&mut self, status: &str, error: Option<String>) -> Result<()> {
        let tools: BTreeMap<_, _> = TOOLS.iter().map(|name| {
            let counts = self.calls.get(*name).copied().unwrap_or_default();
            (*name, json!({"successful_calls":counts.0,"error_calls":counts.1,"negative_scenario":self.negative_cases.contains(*name)}))
        }).collect();
        let report = json!({"status": status, "error": error, "passed": self.passed, "tools":tools,"owned_pids": self.owned_pids, "evidence": self.output, "coverage": "real MCP stdio client, real application, local packs and GPU; no mock backend", "manual_check": {"status":"pending","retained_instance":self.manual_handoff,"criterion":"After disconnect, independently verify normal keyboard movement and quit in a retained window; automated checks establish survival and isolation."}});
        fs::write(
            self.output.join("results.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("{status}: {}", self.output.display());
        Ok(())
    }

    fn kill_owned(&self, pid: u32, signal: &str) -> Result<()> {
        let instance = self
            .owned_pids
            .get(&pid)
            .context("refusing to signal unowned PID")?;
        ensure!(
            !process_alive(pid) || owns_process(pid, instance),
            "refusing to signal reused PID {pid}"
        );
        let status = std::process::Command::new("kill")
            .args([signal, &pid.to_string()])
            .status()?;
        ensure!(
            status.success() || !process_alive(pid),
            "could not signal owned PID {pid}"
        );
        Ok(())
    }

    fn cleanup(&self) -> Result<()> {
        for (pid, instance) in &self.owned_pids {
            if owns_process(*pid, instance) {
                self.kill_owned(*pid, "-TERM")?;
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self
            .owned_pids
            .iter()
            .any(|(pid, instance)| owns_process(*pid, instance))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        for (pid, instance) in &self.owned_pids {
            if owns_process(*pid, instance) {
                self.kill_owned(*pid, "-KILL")?;
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while self
            .owned_pids
            .iter()
            .any(|(pid, instance)| owns_process(*pid, instance))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        ensure!(
            !self
                .owned_pids
                .iter()
                .any(|(pid, instance)| owns_process(*pid, instance)),
            "test-owned windows remain alive; private database retained"
        );
        if let Some(database) = &self.private_database {
            let retained = self.manual_handoff.is_some();
            if !retained {
                let directory = database.parent().context("private database directory")?;
                if directory.exists() {
                    fs::remove_dir_all(directory)?;
                }
            }
            fs::write(
                self.output.join("database-cleanup.json"),
                serde_json::to_vec_pretty(
                    &json!({"private_database":database,"status":if retained {"retained_for_manual_handoff"} else {"removed"},"retained_instance":self.manual_handoff}),
                )?,
            )?;
        }
        Ok(())
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("MCP verifier cleanup: {error:#}");
        }
    }
}

struct Client {
    service: RunningService<RoleClient, ()>,
    child: Child,
    schemas: BTreeMap<String, Value>,
}

impl Client {
    async fn start(config: &Config, session: &str) -> Result<Self> {
        let stderr = File::create(config.output.join(format!("{session}-server.log")))?;
        let mut command = Command::new(&config.server);
        command
            .args([
                "--sandbox-binary",
                config.sandbox.to_str().context("sandbox path")?,
                "--packs-dir",
                config.packs.to_str().context("packs path")?,
                "--runs-dir",
                config.output.to_str().context("output path")?,
            ])
            .current_dir(&config.root)
            .env("NADES_SANDBOX_DATA", &config.database)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .kill_on_drop(true);
        if session == "without-ffmpeg" {
            command.env("PATH", "");
        }
        let mut child = command
            .spawn()
            .context("launch actual nades-mcp executable")?;
        let output = child.stdout.take().context("MCP stdout")?;
        let input = child.stdin.take().context("MCP stdin")?;
        let service = tokio::time::timeout(Duration::from_secs(15), ().serve((output, input)))
            .await
            .context("MCP initialization deadline")??;
        let schemas = service
            .list_all_tools()
            .await?
            .into_iter()
            .filter_map(|tool| {
                tool.output_schema
                    .map(|schema| (tool.name.into_owned(), Value::Object((*schema).clone())))
            })
            .collect();
        Ok(Self {
            service,
            child,
            schemas,
        })
    }

    async fn call(
        &self,
        evidence: &mut Evidence,
        tool: &str,
        arguments: Value,
    ) -> Result<CallToolResult> {
        let request = CallToolRequestParams::new(tool.to_owned()).with_arguments(
            arguments
                .as_object()
                .context("tool arguments must be an object")?
                .clone(),
        );
        let result = tokio::time::timeout(Duration::from_secs(75), self.service.call_tool(request))
            .await
            .with_context(|| format!("{tool}: MCP deadline"))??;
        evidence.record(tool, &arguments, &result)?;
        self.validate_result(tool, &result)?;
        Ok(result)
    }

    fn validate_result(&self, tool: &str, result: &CallToolResult) -> Result<()> {
        let schema = self
            .schemas
            .get(tool)
            .with_context(|| format!("{tool}: missing advertised output schema"))?;
        let data = result
            .structured_content
            .as_ref()
            .with_context(|| format!("{tool}: missing structured result"))?;
        let validator = jsonschema::validator_for(schema)
            .map_err(|error| anyhow::anyhow!("{tool}: invalid advertised schema: {error}"))?;
        validator.validate(data).map_err(|error| {
            anyhow::anyhow!("{tool}: response violates advertised schema: {error}")
        })?;
        Ok(())
    }

    async fn cancel_render(&self, evidence: &mut Evidence, camera: &CameraPose) -> Result<()> {
        let arguments = json!({"map":"de_cache","camera":camera,"width":4096,"height":4096});
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new("view_render")
                .with_arguments(arguments.as_object().unwrap().clone()),
        ));
        let handle = self
            .service
            .send_request_with_option(
                request,
                PeerRequestOptions::with_timeout(Duration::from_secs(10)),
            )
            .await?;
        let server_pid = self.child.id().context("owned MCP server PID")?;
        let mut worker = None;
        for _ in 0..200 {
            worker = find_render_worker(server_pid, &evidence.output);
            if worker.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let worker =
            worker.context("could not observe an owned offscreen worker before cancellation")?;
        self.service
            .notify_cancelled(CancelledNotificationParam::new(
                Some(handle.id.clone()),
                Some("acceptance verifier cancels an observed in-flight headless render".into()),
            ))
            .await?;
        serde_json::to_writer(
            &mut evidence.records,
            &json!({"notification":"notifications/cancelled","request_id":handle.id,"owned_worker_pid":worker}),
        )?;
        evidence.records.write_all(b"\n")?;
        let response = tokio::time::timeout(Duration::from_secs(5), handle.await_response())
            .await
            .context("cancellation did not complete within five seconds")?;
        evidence.check(
            "MCP cancellation completes the client request as cancelled",
            matches!(response, Err(ServiceError::Cancelled { .. })),
        )?;
        evidence.check(
            "MCP cancellation terminates its observed headless worker",
            wait_dead(worker).await,
        )?;
        Ok(())
    }

    async fn ok(
        &self,
        evidence: &mut Evidence,
        tool: &str,
        arguments: Value,
    ) -> Result<CallToolResult> {
        let result = self.call(evidence, tool, arguments).await?;
        ensure!(
            result.is_error != Some(true),
            "{tool}: {}",
            result.structured_content.as_ref().unwrap_or(&Value::Null)
        );
        ensure!(
            result.structured_content.is_some(),
            "{tool}: no structured result"
        );
        Ok(result)
    }

    async fn error(&self, evidence: &mut Evidence, tool: &str, arguments: Value) -> Result<Value> {
        let result = self.call(evidence, tool, arguments).await?;
        ensure!(
            result.is_error == Some(true),
            "{tool}: invalid request was reported as success"
        );
        let data = result
            .structured_content
            .context("tool error must include structured content")?;
        ensure!(
            find_value(&data, "code").and_then(Value::as_str).is_some(),
            "{tool}: error has no stable code"
        );
        Ok(data)
    }

    async fn settled_state(&self, evidence: &mut Evidence) -> Result<AppState> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut previous: Option<AppState> = None;
        loop {
            let current = state(&self.ok(evidence, "app_status", json!({})).await?)?;
            let content_ready = !current.scene.loading
                && !current.scene.playing
                && (current.scene.lineup_id.is_none() || current.scene.throw_id.is_some());
            if content_ready
                && previous.as_ref().is_some_and(|old| {
                    old.instance_id == current.instance_id
                        && old.map == current.map
                        && old.camera == current.camera
                        && old.scene == current.scene
                        && old.viewport == current.viewport
                        && old.scene_viewport == current.scene_viewport
                        && old.revision == current.revision
                })
            {
                evidence.check(
                    "asynchronous M3 startup settles before scene assertions",
                    true,
                )?;
                return Ok(current);
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "application scene did not settle within 60 seconds; latest scene: {:?}",
                current.scene
            );
            previous = Some(current);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn disconnect(mut self) -> Result<()> {
        self.service
            .close_with_timeout(Duration::from_secs(5))
            .await?;
        tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .context("MCP server did not exit after disconnect")??;
        Ok(())
    }
}

struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_png(bytes: &[u8]) -> Result<DecodedPng> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().context("PNG output size")?];
    let info = reader.next_frame(&mut buffer)?;
    let buffer = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer.to_vec(),
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => bail!("expected RGB/RGBA capture, received {other:?}"),
    };
    Ok(DecodedPng {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn image_difference(left: &DecodedPng, right: &DecodedPng) -> Result<Value> {
    ensure!(
        left.width == right.width && left.height == right.height,
        "image dimensions differ"
    );
    let differences: Vec<_> = left
        .rgba
        .iter()
        .zip(&right.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    Ok(
        json!({"width":left.width,"height":left.height,"channels":differences.len(),"changed_channels":differences.iter().filter(|v| **v != 0).count(),"maximum_absolute_channel_error":differences.iter().max(),"mean_absolute_channel_error":differences.iter().map(|v| *v as f64).sum::<f64>() / differences.len() as f64}),
    )
}

fn find_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    let object = value.as_object()?;
    object
        .get(key)
        .or_else(|| object.values().find_map(|v| find_value(v, key)))
}

fn find_pid(value: &Value) -> Option<u32> {
    find_value(value, "pid")
        .and_then(Value::as_u64)
        .and_then(|pid| pid.try_into().ok())
}

fn state(result: &CallToolResult) -> Result<AppState> {
    let data = result
        .structured_content
        .as_ref()
        .context("structured content")?;
    fn decode(value: &Value) -> Option<AppState> {
        serde_json::from_value(value.clone())
            .ok()
            .or_else(|| value.as_object()?.values().find_map(decode))
    }
    decode(data).with_context(|| format!("result lacks an AppState: {data}"))
}

fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn owns_process(pid: u32, instance_id: &str) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .is_ok_and(|output| {
            let command = String::from_utf8_lossy(&output.stdout);
            command.contains("nades-sandbox")
                && command.contains(&format!("--instance-id {instance_id}"))
        })
}

fn find_owned_child(parent: u32, evidence: &Path, marker: &str) -> Option<u32> {
    let processes = std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,command="])
        .output()
        .ok()?;
    String::from_utf8_lossy(&processes.stdout)
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid: u32 = fields.next()?.parse().ok()?;
            (ppid == parent && line.contains(marker) && line.contains(evidence.to_str()?))
                .then_some(pid)
        })
}

fn find_render_worker(parent: u32, evidence: &Path) -> Option<u32> {
    let processes = std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,command="])
        .output()
        .ok()?;
    String::from_utf8_lossy(&processes.stdout)
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid: u32 = fields.next()?.parse().ok()?;
            (ppid == parent && line.contains("--render") && line.contains(evidence.to_str()?))
                .then_some(pid)
        })
}

async fn wait_dead(pid: u32) -> bool {
    for _ in 0..100 {
        if !process_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn pose_matches(actual: &CameraPose, expected: &CameraPose) -> bool {
    actual
        .position
        .iter()
        .zip(expected.position)
        .all(|(a, b)| (*a - b).abs() <= 0.01)
        && (actual.pitch_degrees - expected.pitch_degrees).abs() <= 0.001
        && (actual.yaw_degrees - expected.yaw_degrees).abs() <= 0.001
        && (actual.eye_height_units - expected.eye_height_units).abs() <= 0.001
        && (actual.vertical_fov_degrees - expected.vertical_fov_degrees).abs() <= 0.001
}

fn benchmark_pose(root: &Path) -> Result<CameraPose> {
    let camera: Value = serde_json::from_slice(&fs::read(root.join("bench/camera.json"))?)?;
    let position = camera["position"]
        .as_array()
        .context("benchmark position")?;
    let q: Vec<f64> = camera["quaternion_xyzw"]
        .as_array()
        .context("benchmark quaternion")?
        .iter()
        .map(|v| v.as_f64().context("quaternion number"))
        .collect::<Result<_>>()?;
    let forward_x = -2.0 * (q[0] * q[2] + q[3] * q[1]);
    let forward_y = -2.0 * (q[1] * q[2] - q[3] * q[0]);
    let forward_z = 2.0 * (q[0] * q[0] + q[1] * q[1]) - 1.0;
    Ok(CameraPose {
        position: [
            (position[2].as_f64().context("z")? / 0.0254) as f32,
            (position[0].as_f64().context("x")? / 0.0254) as f32,
            (position[1].as_f64().context("y")? / 0.0254 - 64.0) as f32,
        ],
        pitch_degrees: (-forward_y).asin().to_degrees() as f32,
        yaw_degrees: forward_x.atan2(forward_z).to_degrees() as f32,
        eye_height_units: 64.0,
        vertical_fov_degrees: 73.739_8,
    })
}

fn snapshot_binary(source: &Path, destination: &Path) -> Result<Value> {
    ensure!(
        source.is_file(),
        "unavailable: build executable {} first",
        source.display()
    );
    fs::copy(source, destination)?;
    let hash = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(destination)
        .output()
        .context("hash actual binary snapshot")?;
    ensure!(
        hash.status.success(),
        "could not hash {}",
        destination.display()
    );
    let hash_text = String::from_utf8(hash.stdout)?;
    let sha256 = hash_text
        .split_whitespace()
        .next()
        .context("SHA256 result")?;
    ensure!(
        sha256.len() == 64 && sha256.bytes().all(|c| c.is_ascii_hexdigit()),
        "invalid SHA256 result"
    );
    Ok(json!({"source":source,"snapshot":destination,"sha256":sha256}))
}

fn isolate_database(source: &Path, private: &Path) -> Result<Value> {
    fs::create_dir(private.parent().context("private database parent")?)?;
    let source_metadata = match fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(
                json!({"status":"missing","source":source,"private_path":private,"method":null,"behavior":"NADES_SANDBOX_DATA points to this explicit missing private path; map-only MCP verification remains available."}),
            );
        }
        Err(error) => {
            return Err(error)
                .context("unavailable: cannot inspect source database for private copy");
        }
    };
    ensure!(
        source_metadata.is_file(),
        "unavailable: database source is not a file: {}",
        source.display()
    );
    let copied = std::process::Command::new("cp")
        .arg("-c")
        .arg(source)
        .arg(private)
        .output()
        .context("unavailable: create APFS database clone with cp -c")?;
    ensure!(
        copied.status.success(),
        "unavailable: APFS database clone failed: {}",
        String::from_utf8_lossy(&copied.stderr).trim()
    );
    let private_metadata = fs::metadata(private)?;
    ensure!(
        private_metadata.len() == source_metadata.len(),
        "unavailable: database source changed size during private clone"
    );
    Ok(
        json!({"status":"cloned","source":source,"private_path":private,"method":"APFS cp -c","source_bytes":source_metadata.len(),"private_bytes":private_metadata.len(),"source_opened_with_duckdb":false}),
    )
}

async fn verify(config: &Config, evidence: &mut Evidence) -> Result<()> {
    for path in [&config.server, &config.sandbox] {
        ensure!(
            path.is_file(),
            "unavailable: build executable {} first",
            path.display()
        );
    }
    for map in ["de_cache", "de_mirage"] {
        ensure!(
            config.packs.join(format!("{map}.pack")).is_file(),
            "unavailable: local {map} pack required; run just pack {map}"
        );
    }

    let fixtures = evidence.output.join("synthetic-packs");
    fs::create_dir(&fixtures)?;
    let isolated = Config {
        packs: fixtures.clone(),
        ..config.clone()
    };
    let fixture_client = Client::start(&isolated, "pack-fixtures").await?;
    let empty_maps = fixture_client.ok(evidence, "maps_list", json!({})).await?;
    evidence.check(
        "empty cache reports no maps",
        empty_maps.structured_content.as_ref().unwrap()["maps"] == json!([]),
    )?;
    fs::write(
        fixtures.join("de_corrupt.pack"),
        b"synthetic truncated pack",
    )?;
    let mut incompatible = nades_pack::MAGIC.to_vec();
    incompatible.extend((nades_pack::VERSION + 1).to_le_bytes());
    incompatible.extend(0_u32.to_le_bytes());
    fs::write(fixtures.join("de_incompatible.pack"), incompatible)?;
    let broken_maps = fixture_client.ok(evidence, "maps_list", json!({})).await?;
    let broken = broken_maps.structured_content.as_ref().unwrap()["maps"]
        .as_array()
        .context("maps array")?;
    evidence.check(
        "bad packs are independently classified",
        broken.len() == 2
            && broken[0]["map"] == "de_corrupt"
            && broken[0]["status"] == "corrupt"
            && broken[1]["map"] == "de_incompatible"
            && broken[1]["status"] == "incompatible",
    )?;
    evidence.negative_cases.insert("maps_list".into());
    fixture_client
        .error(evidence, "app_start", json!({"map":"de_missing"}))
        .await?;
    fixture_client
        .error(evidence, "app_start", json!({"map":"de_corrupt"}))
        .await?;
    fixture_client.disconnect().await?;

    let client = Client::start(config, "primary").await?;
    evidence.check(
        "server instructions explain observe-act-verify and headless rendering",
        client.service.peer_info().is_some_and(|info| {
            info.instructions.as_deref().is_some_and(|text| {
                text.contains("app_status")
                    && text.contains("view_render")
                    && text.contains("instance_id")
            })
        }),
    )?;
    let tools = client.service.list_all_tools().await?;
    let actual: BTreeSet<_> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    evidence.check(
        "discover all thirteen tools",
        tools.len() == TOOLS.len() && actual == BTreeSet::from(TOOLS),
    )?;
    evidence.check(
        "all tools have descriptions and input/output schemas",
        tools.iter().all(|tool| {
            tool.description.as_ref().is_some_and(|s| !s.is_empty())
                && tool.input_schema.get("type").and_then(Value::as_str) == Some("object")
                && tool.output_schema.is_some()
        }),
    )?;
    fs::write(
        evidence.output.join("tools.json"),
        serde_json::to_vec_pretty(&tools)?,
    )?;

    let cold = client.ok(evidence, "app_status", json!({})).await?;
    evidence.check(
        "cold status does not start a window",
        state(&cold).is_err() && find_pid(cold.structured_content.as_ref().unwrap()).is_none(),
    )?;
    let cold_data = cold.structured_content.as_ref().unwrap();
    evidence.check(
        "cold status suggests startup",
        cold_data["next_actions"]
            .as_array()
            .is_some_and(|items| items.contains(&json!("app_start"))),
    )?;
    let maps = client.ok(evidence, "maps_list", json!({})).await?;
    let map_text = serde_json::to_string(maps.structured_content.as_ref().unwrap())?;
    evidence.check(
        "map discovery includes installed maps",
        map_text.contains("de_cache") && map_text.contains("de_mirage"),
    )?;
    client
        .error(evidence, "lineups_list", json!({"map":"../invalid"}))
        .await?;
    client
        .error(evidence, "video_status", json!({"job_id":"unknown"}))
        .await?;
    client
        .error(evidence, "video_cancel", json!({"job_id":"unknown"}))
        .await?;
    client
        .error(
            evidence,
            "video_render",
            json!({"map":"../invalid","throw_id":"missing","start_seconds":0,"end_seconds":1}),
        )
        .await?;
    let lineups = client
        .ok(
            evidence,
            "lineups_list",
            json!({"map":"de_dust2","grenade":"molotov","destination":"BombsiteA"}),
        )
        .await?;
    let throw_id = lineups.structured_content.as_ref().unwrap()["lineups"]
        .as_array()
        .and_then(|l| l.first())
        .and_then(|l| l["throw_id"].as_str())
        .context("Dust II A-site Molotov unavailable in private database")?
        .to_string();
    let args = json!({"map":"de_dust2","throw_id":throw_id,"start_seconds":-0.5,"end_seconds":1.5,"width":640,"height":360,"fps":10,"camera_keyframes":[{"at_seconds":0,"camera":{"position":[700,2300,270],"pitch_degrees":18.54,"yaw_degrees":26.565,"eye_height_units":0}},{"at_seconds":2,"camera":{"position":[900,2240,260],"pitch_degrees":17,"yaw_degrees":50,"eye_height_units":0}}]});
    let started = client.ok(evidence, "video_render", args.clone()).await?;
    let job = started.structured_content.as_ref().unwrap()["job_id"]
        .as_str()
        .context("video job ID")?
        .to_string();
    client.error(evidence, "video_render", args.clone()).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let finished = loop {
        let result = client
            .ok(evidence, "video_status", json!({"job_id":job}))
            .await?;
        let data = result.structured_content.unwrap();
        if data["state"] != "running" {
            break data;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "video did not finish"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    ensure!(finished["state"] == "completed", "video failed: {finished}");
    let path = finished["path"].as_str().context("video path")?;
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_frames",
            "-of",
            "json",
            path,
        ])
        .output()
        .context("ffprobe required for video acceptance")?;
    ensure!(probe.status.success(), "ffprobe failed");
    let probe: Value = serde_json::from_slice(&probe.stdout)?;
    let video = &probe["streams"][0];
    evidence.check(
        "headless Molotov video decodes as twenty H.264 frames",
        video["codec_name"] == "h264"
            && video["width"] == 640
            && video["height"] == 360
            && video["nb_read_frames"] == "20",
    )?;
    let cancel_args = json!({"map":"de_dust2","throw_id":throw_id,"start_seconds":0,"end_seconds":30,"width":1280,"height":720,"fps":30});
    let pending = client.ok(evidence, "video_render", cancel_args).await?;
    let cancelled_id = pending.structured_content.as_ref().unwrap()["job_id"]
        .as_str()
        .context("cancellable job ID")?
        .to_string();
    let progress_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = client
            .ok(evidence, "video_status", json!({"job_id":cancelled_id}))
            .await?
            .structured_content
            .unwrap();
        ensure!(
            status["state"] == "running",
            "video ended before cancellation: {status}"
        );
        if status["frames_completed"].as_u64().unwrap_or(0) > 0 {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < progress_deadline,
            "video never began rendering"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let worker_pid = find_render_worker(client.child.id().context("MCP PID")?, &evidence.output)
        .context("active video renderer PID")?;
    let encoder_pid = find_owned_child(
        client.child.id().context("MCP PID")?,
        &evidence.output,
        "partial.mp4",
    )
    .context("active video encoder PID")?;
    client
        .ok(evidence, "video_cancel", json!({"job_id":cancelled_id}))
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let cancelled = loop {
        let result = client
            .ok(evidence, "video_status", json!({"job_id":cancelled_id}))
            .await?
            .structured_content
            .unwrap();
        if result["state"] != "running" {
            break result;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "video cancellation timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    evidence.check(
        "video cancellation cleans its unfinished job",
        cancelled["state"] == "cancelled"
            && cancelled["path"].is_null()
            && wait_dead(worker_pid).await
            && wait_dead(encoder_pid).await
            && !fs::read_dir(&evidence.output)?
                .filter_map(Result::ok)
                .any(|e| {
                    e.file_type().is_ok_and(|t| t.is_dir())
                        && e.file_name().to_string_lossy().starts_with("video-")
                }),
    )?;
    let no_encoder = Client::start(config, "without-ffmpeg").await?;
    let without = no_encoder.ok(evidence, "video_render", json!({"map":"de_dust2","throw_id":throw_id,"start_seconds":0,"end_seconds":1,"width":320,"height":180,"fps":4})).await?;
    let no_encoder_id = without.structured_content.as_ref().unwrap()["job_id"]
        .as_str()
        .context("no encoder job ID")?
        .to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let failure = loop {
        let value = no_encoder
            .ok(evidence, "video_status", json!({"job_id":no_encoder_id}))
            .await?
            .structured_content
            .unwrap();
        if value["state"] != "running" {
            break value;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "missing encoder failed to resolve"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    evidence.check(
        "missing encoder reports actionable failure",
        failure["state"] == "failed"
            && failure["error"]
                .as_str()
                .is_some_and(|s| s.contains("install ffmpeg")),
    )?;
    no_encoder.disconnect().await?;
    let lost = client.ok(evidence, "video_render", json!({"map":"de_dust2","throw_id":"missing-throw","start_seconds":0,"end_seconds":1,"width":320,"height":180,"fps":4})).await?;
    let lost_id = lost.structured_content.as_ref().unwrap()["job_id"]
        .as_str()
        .context("missing throw job ID")?
        .to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(70);
    let failure = loop {
        let value = client
            .ok(evidence, "video_status", json!({"job_id":lost_id}))
            .await?
            .structured_content
            .unwrap();
        if value["state"] != "running" {
            break value;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "missing throw failed to resolve"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    evidence.check(
        "unknown throw reports renderer failure",
        failure["state"] == "failed"
            && failure["error"]
                .as_str()
                .is_some_and(|s| s.contains("no throw")),
    )?;
    let disconnected = Client::start(config, "video-disconnect").await?;
    let pending = disconnected.ok(evidence, "video_render", json!({"map":"de_dust2","throw_id":throw_id,"start_seconds":0,"end_seconds":30,"width":640,"height":360,"fps":30})).await?;
    let lost_job = pending.structured_content.as_ref().unwrap()["job_id"]
        .as_str()
        .context("disconnect job ID")?
        .to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = disconnected
            .ok(evidence, "video_status", json!({"job_id":lost_job}))
            .await?
            .structured_content
            .unwrap();
        ensure!(
            status["state"] == "running",
            "video ended before disconnect: {status}"
        );
        if status["frames_completed"].as_u64().unwrap_or(0) > 0 {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "disconnect test never began rendering"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let parent = disconnected.child.id().context("disconnect server PID")?;
    let renderer =
        find_render_worker(parent, &evidence.output).context("disconnect renderer PID")?;
    let encoder = find_owned_child(parent, &evidence.output, "partial.mp4")
        .context("disconnect encoder PID")?;
    disconnected.disconnect().await?;
    evidence.check(
        "disconnect reaps the active video renderer and encoder",
        wait_dead(renderer).await
            && wait_dead(encoder).await
            && !fs::read_dir(&evidence.output)?
                .filter_map(Result::ok)
                .any(|e| {
                    e.file_type().is_ok_and(|t| t.is_dir())
                        && e.file_name().to_string_lossy().starts_with("video-")
                }),
    )?;
    let cold_render = client.ok(evidence, "view_render", json!({"map":"de_cache","camera":benchmark_pose(&config.root)?,"width":320,"height":180})).await?;
    let cold_image = evidence.image(&cold_render, "cold-headless-map")?;
    let render_marks = cold_render.structured_content.as_ref().unwrap()["startup_marks"]["marks"]
        .as_array()
        .context("headless render stage marks")?;
    evidence.check(
        "headless screenshot works before app_start without window stages",
        cold_image.width == 320
            && cold_image.height == 180
            && render_marks.iter().all(|mark| {
                !matches!(
                    mark[0].as_str(),
                    Some("window" | "event_loop" | "frame:presented")
                )
            }),
    )?;
    let still_cold = client.ok(evidence, "app_status", json!({})).await?;
    evidence.check(
        "cold headless rendering leaves the manager stopped",
        state(&still_cold).is_err()
            && find_pid(still_cold.structured_content.as_ref().unwrap()).is_none(),
    )?;
    client
        .error(evidence, "app_stop", json!({"instance_id":"not-owned"}))
        .await?;
    client
        .error(
            evidence,
            "frame_capture",
            json!({"instance_id":"not-owned"}),
        )
        .await?;

    let started = client
        .ok(evidence, "app_start", json!({"map":"de_cache"}))
        .await?;
    let initial = state(&started)?;
    let pid = find_pid(started.structured_content.as_ref().unwrap())
        .context("app_start must expose owned PID")?;
    evidence.check(
        "start confirms presented visible requested map",
        initial.map == "de_cache"
            && initial.viewport.visible
            && initial.frame_id > 0
            && process_alive(pid),
    )?;
    let ready_data = started.structured_content.as_ref().unwrap();
    if cfg!(target_os = "macos") {
        let memory = initial
            .metrics
            .memory
            .as_ref()
            .context("macOS process memory is unavailable")?;
        evidence.check(
            "status reports measured memory in bytes",
            memory.physical_footprint_bytes > 0
                && memory.peak_physical_footprint_bytes >= memory.physical_footprint_bytes,
        )?;
    }
    evidence.check(
        "ready status explains shown scene and next tools",
        ready_data["summary"]
            .as_str()
            .is_some_and(|text| text.contains("de_cache"))
            && ready_data["next_actions"]
                .as_array()
                .is_some_and(|items| items.contains(&json!("frame_capture"))),
    )?;
    let repeated = client
        .ok(evidence, "app_start", json!({"map":"de_cache"}))
        .await?;
    evidence.check(
        "repeated start reuses the same child",
        state(&repeated)?.instance_id == initial.instance_id
            && find_pid(repeated.structured_content.as_ref().unwrap()) == Some(pid),
    )?;
    client
        .error(evidence, "app_start", json!({"map":"de_mirage"}))
        .await?;
    let settled = client.settled_state(evidence).await?;
    let smoke_list = client
        .ok(
            evidence,
            "lineups_list",
            json!({"map":"de_dust2","grenade":"smoke"}),
        )
        .await?;
    let smoke_id = smoke_list.structured_content.as_ref().unwrap()["lineups"]
        .as_array()
        .and_then(|l| l.first())
        .and_then(|l| l["throw_id"].as_str())
        .context("Dust II smoke unavailable")?
        .to_string();
    let smoke = client.ok(evidence, "video_render", json!({"map":"de_dust2","throw_id":smoke_id,"start_seconds":1,"end_seconds":2,"width":320,"height":180,"fps":4})).await?;
    let smoke_job = smoke.structured_content.as_ref().unwrap()["job_id"]
        .as_str()
        .context("smoke job ID")?
        .to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let smoke_result = loop {
        let status = client
            .ok(evidence, "video_status", json!({"job_id":smoke_job}))
            .await?
            .structured_content
            .unwrap();
        if status["state"] != "running" {
            break status;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "smoke video timed out"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    ensure!(
        smoke_result["state"] == "completed",
        "smoke video failed: {smoke_result}"
    );
    let after_smoke = state(&client.ok(evidence, "app_status", json!({})).await?)?;
    evidence.check(
        "fixed-camera smoke video preserves the live view",
        after_smoke.map == settled.map
            && after_smoke.camera == settled.camera
            && after_smoke.revision == settled.revision,
    )?;
    let ui = client
        .ok(
            evidence,
            "frame_capture",
            json!({"instance_id":initial.instance_id}),
        )
        .await?;
    let ui_image = evidence.image(&ui, "window-with-ui")?;
    evidence.check(
        "default capture is a native-sized PNG",
        ui_image.width == settled.viewport.width && ui_image.height == settled.viewport.height,
    )?;

    let pose_a = benchmark_pose(&config.root)?;
    let mut pose_b = pose_a.clone();
    pose_b.position[0] += 120.0;
    pose_b.position[1] += 80.0;
    pose_b.yaw_degrees = (pose_b.yaw_degrees + 25.0 + 180.0).rem_euclid(360.0) - 180.0;
    let mut captures = Vec::new();
    for (name, pose) in [("camera-a", &pose_a), ("camera-b", &pose_b)] {
        let update = client
            .ok(
                evidence,
                "camera_set",
                json!({"instance_id":initial.instance_id,"camera":pose}),
            )
            .await?;
        let applied = state(&update)?;
        evidence.check(
            &format!("{name}: camera readback matches CS2 pose"),
            pose_matches(&applied.camera, pose),
        )?;
        let capture = client.ok(evidence, "frame_capture", json!({"instance_id":initial.instance_id,"include_ui":false,"expected_revision":applied.revision})).await?;
        let image = evidence.image(&capture, name)?;
        let captured = state(&capture)?;
        evidence.check(
            &format!("{name}: scene capture uses its actual image dimensions"),
            capture.structured_content.as_ref().unwrap()["image"]["width"] == image.width
                && capture.structured_content.as_ref().unwrap()["image"]["height"] == image.height,
        )?;
        evidence.check(
            &format!("{name}: capture identifies applied revision"),
            captured.instance_id == initial.instance_id
                && captured.revision == applied.revision
                && captured.presented_revision == captured.revision
                && captured.frame_id >= applied.frame_id
                && pose_matches(&captured.camera, pose),
        )?;
        captures.push((captured, image));
    }
    evidence.check(
        "distinct camera poses produce distinct scene pixels",
        captures[0].1.rgba != captures[1].1.rgba,
    )?;
    let (captured, image) = &captures[1];
    evidence.check(
        "pixel comparison is a settled scene with no active effects",
        !captured.scene.loading
            && !captured.scene.playing
            && captured.scene.active_effects.is_empty(),
    )?;
    let render_args = json!({"map":"de_cache","camera":captured.camera,"width":image.width,"height":image.height,"render_time_seconds":captured.render_time_seconds});
    let render = client
        .ok(evidence, "view_render", render_args.clone())
        .await?;
    let rendered = evidence.image(&render, "headless-matching-camera")?;
    evidence.check(
        "headless dimensions match request",
        rendered.width == image.width && rendered.height == image.height,
    )?;
    ensure!(
        rendered.rgba.len() == image.rgba.len(),
        "decoded image lengths differ"
    );
    let difference = image
        .rgba
        .iter()
        .zip(&rendered.rgba)
        .map(|(a, b)| a.abs_diff(*b) as f64)
        .sum::<f64>()
        / image.rgba.len() as f64;
    fs::write(
        evidence.output.join("image-comparison.json"),
        serde_json::to_vec_pretty(
            &json!({"mean_absolute_channel_error":difference,"maximum":1.0,"render_time_seconds":captured.render_time_seconds}),
        )?,
    )?;
    evidence.check(
        "live scene and headless image differ by at most one display level",
        difference <= 1.0,
    )?;
    let repeat = client.ok(evidence, "view_render", render_args).await?;
    let repeated_image = evidence.image(&repeat, "headless-repeat")?;
    let repeat_difference = image_difference(&rendered, &repeated_image)?;
    fs::write(
        evidence.output.join("repeat-comparison.json"),
        serde_json::to_vec_pretty(&repeat_difference)?,
    )?;
    evidence.check(
        "fixed-input headless renders reproduce within one quantization level",
        repeat_difference["maximum_absolute_channel_error"]
            .as_u64()
            .is_some_and(|value| value <= 1)
            && repeat_difference["mean_absolute_channel_error"]
                .as_f64()
                .is_some_and(|value| value <= 0.001),
    )?;
    let after_render = client.ok(evidence, "app_status", json!({})).await?;
    let unchanged = state(&after_render)?;
    evidence.check(
        "headless rendering preserves live scene",
        unchanged.instance_id == captured.instance_id
            && unchanged.map == captured.map
            && unchanged.revision == captured.revision
            && pose_matches(&unchanged.camera, &captured.camera),
    )?;
    client.cancel_render(evidence, &pose_a).await?;
    let after_cancel = state(&client.ok(evidence, "app_status", json!({})).await?)?;
    evidence.check(
        "headless cancellation preserves the live scene",
        after_cancel.instance_id == captured.instance_id
            && after_cancel.revision == captured.revision
            && pose_matches(&after_cancel.camera, &captured.camera),
    )?;
    let temporary_render_outputs = fs::read_dir(&evidence.output)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("render-"))
        .count();
    evidence.check(
        "headless worker temporary outputs are cleaned",
        temporary_render_outputs == 0,
    )?;

    client
        .error(
            evidence,
            "camera_set",
            json!({"instance_id":initial.instance_id,"camera":pose_a,"expected_revision":0}),
        )
        .await?;
    let mut invalid_camera = pose_a.clone();
    invalid_camera.pitch_degrees = 90.0;
    client
        .error(
            evidence,
            "camera_set",
            json!({"instance_id":initial.instance_id,"camera":invalid_camera}),
        )
        .await?;
    client
        .error(
            evidence,
            "frame_capture",
            json!({"instance_id":initial.instance_id,"expected_revision":0}),
        )
        .await?;
    client
        .error(
            evidence,
            "view_render",
            json!({"map":"de_cache","camera":pose_a,"width":0}),
        )
        .await?;
    client
        .error(
            evidence,
            "view_render",
            json!({"map":"de_cache","camera":pose_a,"width":4097}),
        )
        .await?;
    client
        .error(evidence, "diagnostics_get", json!({"limit":201}))
        .await?;
    let missing_map = format!(
        "de_mcp_missing_{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    client
        .error(
            evidence,
            "map_open",
            json!({"instance_id":initial.instance_id,"map":missing_map}),
        )
        .await?;
    let preserved = client.ok(evidence, "app_status", json!({})).await?;
    evidence.check(
        "invalid requests preserve current scene",
        state(&preserved)?.instance_id == initial.instance_id
            && pose_matches(&state(&preserved)?.camera, &captured.camera)
            && process_alive(pid),
    )?;
    let diagnostics = client
        .ok(evidence, "diagnostics_get", json!({"limit":3}))
        .await?;
    let diagnostics_text = serde_json::to_string(diagnostics.structured_content.as_ref().unwrap())?;
    evidence.check(
        "diagnostics expose metrics and startup marks without image data",
        diagnostics_text.contains("startup_marks")
            && diagnostics_text.contains("metrics")
            && !diagnostics_text.contains("png_base64"),
    )?;
    let diagnostic_data = diagnostics.structured_content.as_ref().unwrap();
    evidence.check(
        "diagnostic records honor requested limit",
        diagnostic_data["logs"]
            .as_array()
            .is_some_and(|logs| logs.len() <= 3),
    )?;
    evidence.check(
        "diagnostics explain metrics and next operations",
        diagnostic_data["metric_definitions"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
            && diagnostic_data["next_actions"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
    )?;
    evidence.check(
        "frame metrics contain measured CPU samples",
        captured.metrics.samples > 0
            && captured
                .metrics
                .median_cpu_ms
                .is_some_and(|ms| ms.is_finite() && ms >= 0.0)
            && captured
                .metrics
                .p95_cpu_ms
                .is_some_and(|ms| ms.is_finite() && ms >= 0.0),
    )?;

    let switched = client
        .ok(
            evidence,
            "map_open",
            json!({"instance_id":initial.instance_id,"map":"de_mirage"}),
        )
        .await?;
    let replacement = state(&switched)?;
    let replacement_pid =
        find_pid(switched.structured_content.as_ref().unwrap()).context("replacement PID")?;
    evidence.check(
        "map restart replaces identity and presents requested map",
        replacement.map == "de_mirage"
            && replacement.instance_id != initial.instance_id
            && replacement.frame_id > 0
            && replacement.viewport.visible
            && wait_dead(pid).await
            && process_alive(replacement_pid),
    )?;
    client
        .error(
            evidence,
            "camera_set",
            json!({"instance_id":initial.instance_id,"camera":pose_a}),
        )
        .await?;
    client
        .error(
            evidence,
            "app_stop",
            json!({"instance_id":initial.instance_id}),
        )
        .await?;
    evidence.check(
        "stale operations leave replacement alive",
        process_alive(replacement_pid),
    )?;

    evidence.kill_owned(replacement_pid, "-KILL")?;
    let crashed = client.ok(evidence, "app_status", json!({})).await?;
    evidence.check(
        "status detects external child failure",
        state(&crashed).is_err(),
    )?;
    evidence.negative_cases.insert("app_status".into());
    let crash_diagnostics = client
        .ok(evidence, "diagnostics_get", json!({"limit":5}))
        .await?;
    evidence.check(
        "child failure remains diagnosable",
        crash_diagnostics.structured_content.as_ref().unwrap()["last_error"]["code"]
            .as_str()
            .is_some_and(|code| !code.is_empty()),
    )?;
    client
        .error(
            evidence,
            "frame_capture",
            json!({"instance_id":replacement.instance_id}),
        )
        .await?;
    let recovered = client
        .ok(evidence, "app_start", json!({"map":"de_cache"}))
        .await?;
    let recovered_state = state(&recovered)?;
    let recovered_pid =
        find_pid(recovered.structured_content.as_ref().unwrap()).context("recovery PID")?;
    evidence.check(
        "start recovers after external failure",
        recovered_state.instance_id != replacement.instance_id && recovered_state.viewport.visible,
    )?;
    client
        .ok(
            evidence,
            "app_stop",
            json!({"instance_id":recovered_state.instance_id}),
        )
        .await?;
    client
        .ok(
            evidence,
            "app_stop",
            json!({"instance_id":recovered_state.instance_id}),
        )
        .await?;
    evidence.check(
        "stop is idempotent and closes the owned process",
        wait_dead(recovered_pid).await,
    )?;

    let handoff = client
        .ok(evidence, "app_start", json!({"map":"de_cache"}))
        .await?;
    let handoff_state = state(&handoff)?;
    let handoff_pid =
        find_pid(handoff.structured_content.as_ref().unwrap()).context("handoff PID")?;
    client.disconnect().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    evidence.check(
        "ready window survives MCP disconnect",
        process_alive(handoff_pid),
    )?;
    let later = Client::start(config, "later").await?;
    let later_status = later.ok(evidence, "app_status", json!({})).await?;
    evidence.check(
        "new session does not adopt handed-off window",
        state(&later_status).is_err(),
    )?;
    later
        .error(
            evidence,
            "app_stop",
            json!({"instance_id":handoff_state.instance_id}),
        )
        .await?;
    evidence.check(
        "new session cannot stop handed-off window",
        process_alive(handoff_pid),
    )?;
    later.disconnect().await?;
    evidence.check(
        "all thirteen tools have positive and negative scenarios",
        TOOLS.iter().all(|tool| {
            evidence.calls.get(*tool).is_some_and(|counts| counts.0 > 0)
                && evidence.negative_cases.contains(*tool)
        }),
    )?;
    if config.keep_handoff {
        evidence.owned_pids.remove(&handoff_pid);
        evidence.manual_handoff = Some((handoff_pid, handoff_state.instance_id.clone()));
        println!(
            "MANUAL CHECK: retained nades-sandbox PID {handoff_pid}, instance {}. Verify W movement and quit; then record the result in {}.",
            handoff_state.instance_id,
            evidence.output.display()
        );
    } else {
        evidence.kill_owned(handoff_pid, "-TERM")?;
        evidence.check(
            "verifier cleans its retained window",
            wait_dead(handoff_pid).await,
        )?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments
        .get(1)
        .is_some_and(|arg| arg == "--compare-images")
    {
        ensure!(
            arguments.len() == 4,
            "--compare-images requires LEFT.png RIGHT.png"
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&image_difference(
                &decode_png(&fs::read(&arguments[2])?)?,
                &decode_png(&fs::read(&arguments[3])?)?
            )?)?
        );
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--help") {
        println!(
            "Verify the real nades MCP, window and offscreen renderer.\n\nOptions: --server PATH --sandbox PATH --packs-dir PATH --output NEW_DIRECTORY --keep-handoff\n\nThe default run cleans all test windows. --keep-handoff retains the last window for manual movement/quit verification."
        );
        return Ok(());
    }
    let mut config = Config::parse()?;
    let mut evidence = Evidence::new(config.output.clone())?;
    let result = async {
        let binaries = evidence.output.join("bin");
        fs::create_dir(&binaries)?;
        let server_snapshot = binaries.join("nades-mcp");
        let sandbox_snapshot = binaries.join("nades-sandbox");
        let server = snapshot_binary(&config.server, &server_snapshot)?;
        let sandbox = snapshot_binary(&config.sandbox, &sandbox_snapshot)?;
        config.server = server_snapshot;
        config.sandbox = sandbox_snapshot;
        // the cs2-throws file the app reads (the first of NADES_SANDBOX_DATA), cloned; without one, map-only
        let database_source = std::env::var_os("NADES_SANDBOX_DATA").and_then(|v| std::env::split_paths(&v).next()).unwrap_or_else(|| evidence.output.join("no-data.sqlite"));
        let database_source = if database_source.is_absolute() { database_source } else { config.root.join(database_source) };
        config.database = evidence.output.join("private-data/throws.sqlite");
        evidence.private_database = Some(config.database.clone());
        let database = isolate_database(&database_source, &config.database)?;
        fs::write(
            evidence.output.join("environment.json"),
            serde_json::to_vec_pretty(
                &json!({"server":server,"sandbox":sandbox,"database":database,"packs":config.packs,"platform":std::env::consts::OS,"load_average":std::process::Command::new("sysctl").args(["-n","vm.loadavg"]).output().ok().map(|v| String::from_utf8_lossy(&v.stdout).trim().to_owned())}),
            )?,
        )?;
        verify(&config, &mut evidence).await
    }.await;
    let result = result.and(evidence.cleanup());
    match result {
        Ok(()) => evidence.finish("passed", None),
        Err(error) => {
            let status = if error.to_string().contains("unavailable:") {
                "unavailable"
            } else {
                "failed"
            };
            evidence.finish(status, Some(format!("{error:#}")))?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apfs_database_clone_has_independent_contents() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.duckdb");
        let private = directory.path().join("private/nades.duckdb");
        fs::write(&source, b"synthetic database copy fixture").unwrap();
        let provenance = isolate_database(&source, &private).unwrap();
        assert_eq!(provenance["status"], "cloned");
        assert_eq!(
            fs::read(&private).unwrap(),
            b"synthetic database copy fixture"
        );
        fs::write(&private, b"changed only in the private clone").unwrap();
        assert_eq!(
            fs::read(&source).unwrap(),
            b"synthetic database copy fixture"
        );
    }

    #[test]
    fn absent_database_uses_an_explicit_missing_private_path() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("absent.duckdb");
        let private = directory.path().join("private/nades.duckdb");
        let provenance = isolate_database(&source, &private).unwrap();
        assert_eq!(provenance["status"], "missing");
        assert_eq!(
            provenance["private_path"],
            serde_json::to_value(&private).unwrap()
        );
        assert!(!private.exists());
    }
}

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{extract::State, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Default)]
struct MusicState {
    queue: VecDeque<Track>,
    current: Option<Track>,
    current_started_at: u64,
    player: Option<Child>,
    history: HashMap<String, u64>,
    loop_enabled: bool,
    shuffle_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Track {
    source: String,
    video: bool,
}

#[derive(Deserialize)]
struct ControlRequest {
    action: String,
    #[serde(default)]
    payload: String,
}

#[derive(Serialize)]
struct ControlResponse {
    ok: bool,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("music_worker=info".parse()?),
        )
        .init();

    let state = Arc::new(Mutex::new(MusicState::default()));
    let app = Router::new()
        .route("/v1/control", post(control_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:9475").await?;
    tracing::info!("music worker at http://127.0.0.1:9475");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn control_handler(
    State(state): State<Arc<Mutex<MusicState>>>,
    Json(request): Json<ControlRequest>,
) -> Json<ControlResponse> {
    let response = match request.action.as_str() {
        "play" => enqueue(Arc::clone(&state), request.payload, false).await,
        "vplay" => enqueue(Arc::clone(&state), request.payload, true).await,
        "queue" => queue_response(&state).await,
        "skip" => skip(Arc::clone(&state)).await,
        "seek" => seek(Arc::clone(&state), request.payload).await,
        "loop" => {
            let mut state = state.lock().await;
            state.loop_enabled = parse_toggle(&request.payload, state.loop_enabled);
            ControlResponse::ok(format!(
                "loop {}",
                if state.loop_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ))
        }
        "shuffle" => {
            let mut state = state.lock().await;
            state.shuffle_enabled = parse_toggle(&request.payload, state.shuffle_enabled);
            ControlResponse::ok(format!(
                "shuffle {}",
                if state.shuffle_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ))
        }
        "stop" => {
            let mut state = state.lock().await;
            stop_player(&mut state).await;
            state.current = None;
            state.queue.clear();
            ControlResponse::ok("stopped")
        }
        "toptracks" => {
            let state = state.lock().await;
            toptracks(&state)
        }
        _ => ControlResponse::err("unknown action"),
    };
    Json(response)
}

async fn enqueue(state: Arc<Mutex<MusicState>>, source: String, video: bool) -> ControlResponse {
    let source = source.trim().to_string();
    if source.is_empty() {
        return ControlResponse::err("empty source");
    }
    let track = Track { source, video };
    let mut state = state.lock().await;
    *state.history.entry(track.source.clone()).or_default() += 1;
    if state.current.is_none() {
        match start_track(&mut state, track, 0).await {
            Ok(()) => ControlResponse::ok("playing through ffmpeg sink"),
            Err(error) => ControlResponse::err(error.to_string()),
        }
    } else {
        state.queue.push_back(track);
        ControlResponse::ok("queued")
    }
}

async fn queue_response(state: &Arc<Mutex<MusicState>>) -> ControlResponse {
    let state = state.lock().await;
    let mut lines = Vec::new();
    if let Some(current) = &state.current {
        let pid = state
            .player
            .as_ref()
            .and_then(|child| child.id())
            .unwrap_or(0);
        lines.push(format!(
            "now: {} [{}] pid={pid}",
            current.source,
            if current.video { "video" } else { "audio" }
        ));
    }
    for (index, track) in state.queue.iter().enumerate() {
        lines.push(format!("{}. {}", index + 1, track.source));
    }
    if lines.is_empty() {
        lines.push("empty".to_string());
    }
    ControlResponse::text("queue", lines.join("\n"))
}

async fn skip(state: Arc<Mutex<MusicState>>) -> ControlResponse {
    let mut state = state.lock().await;
    stop_player(&mut state).await;
    if state.loop_enabled {
        if let Some(current) = state.current.clone() {
            state.queue.push_back(current);
        }
    }
    let Some(next) = state.queue.pop_front() else {
        state.current = None;
        return ControlResponse::ok("queue empty");
    };
    match start_track(&mut state, next, 0).await {
        Ok(()) => ControlResponse::ok("skipped"),
        Err(error) => ControlResponse::err(error.to_string()),
    }
}

async fn seek(state: Arc<Mutex<MusicState>>, payload: String) -> ControlResponse {
    let offset = match parse_seconds(&payload) {
        Some(value) => value,
        None => return ControlResponse::err("seek payload must be seconds or mm:ss"),
    };
    let mut state = state.lock().await;
    let Some(track) = state.current.clone() else {
        return ControlResponse::err("nothing is playing");
    };
    stop_player(&mut state).await;
    match start_track(&mut state, track, offset).await {
        Ok(()) => ControlResponse::ok(format!("seeked to {offset}s")),
        Err(error) => ControlResponse::err(error.to_string()),
    }
}

async fn start_track(state: &mut MusicState, track: Track, offset: u64) -> Result<()> {
    let input = resolve_source(&track.source).await?;
    let mut command = Command::new("ffmpeg");
    command.arg("-hide_banner").arg("-loglevel").arg("warning");
    if offset > 0 {
        command.arg("-ss").arg(offset.to_string());
    }
    command.arg("-re").arg("-i").arg(input);
    if track.video {
        command
            .args([
                "-f", "matroska", "-codec:v", "libx264", "-codec:a", "libopus",
            ])
            .arg("-");
    } else {
        command
            .args(["-vn", "-f", "s16le", "-ac", "2", "-ar", "48000"])
            .arg("-");
    }
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let child = command.spawn()?;
    state.current = Some(track);
    state.current_started_at = offset;
    state.player = Some(child);
    Ok(())
}

async fn resolve_source(source: &str) -> Result<String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        if source.contains(".m3u8") || source.contains(".mp3") || source.contains(".ogg") {
            return Ok(source.to_string());
        }
    }
    let query = if source.starts_with("http://") || source.starts_with("https://") {
        source.to_string()
    } else {
        format!("ytsearch1:{source}")
    };
    let output = tokio::time::timeout(
        Duration::from_secs(45),
        Command::new("yt-dlp")
            .args(["-f", "bestaudio/best", "--no-playlist", "-g", "--", &query])
            .output(),
    )
    .await??;
    if !output.status.success() {
        anyhow::bail!(
            "yt-dlp failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    url.ok_or_else(|| anyhow::anyhow!("yt-dlp returned no playable URL"))
}

async fn stop_player(state: &mut MusicState) {
    if let Some(mut child) = state.player.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

fn parse_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let (minutes, seconds) = value.split_once(':')?;
    Some(minutes.parse::<u64>().ok()? * 60 + seconds.parse::<u64>().ok()?)
}

fn toptracks(state: &MusicState) -> ControlResponse {
    let mut items = state.history.iter().collect::<Vec<_>>();
    items.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    let text = items
        .into_iter()
        .take(10)
        .enumerate()
        .map(|(index, (track, count))| format!("{}. {} ({count})", index + 1, track))
        .collect::<Vec<_>>()
        .join("\n");
    ControlResponse::text(
        "toptracks",
        if text.is_empty() {
            "No stats yet.".to_string()
        } else {
            text
        },
    )
}

fn parse_toggle(value: &str, current: bool) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => true,
        "off" | "false" | "0" => false,
        _ => !current,
    }
}

impl ControlResponse {
    fn ok(status: impl Into<String>) -> Self {
        Self {
            ok: true,
            status: status.into(),
            text: None,
            error: None,
        }
    }

    fn text(status: impl Into<String>, text: String) -> Self {
        Self {
            ok: true,
            status: status.into(),
            text: Some(text),
            error: None,
        }
    }

    fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            status: "error".to_string(),
            text: None,
            error: Some(error.into()),
        }
    }
}

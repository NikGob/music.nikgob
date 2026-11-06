//! One connection to Soloist's WebSocket API: commands go out as they are
//! queued, every event updates the shared [`Live`] state. The connection
//! ends on any socket error; the daemon decides whether to connect again.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::protocol::{Anchor, Command, Event, Item, Status};

/// What Soloist last said, as the player reads it. Updated by the
/// connection, read by the tracks and the settings.
#[derive(Debug, Default)]
pub struct Live {
    /// The socket is open.
    pub connected: bool,
    pub logged_in: bool,
    /// Soloist is the account's active Spotify Connect device.
    pub active: bool,
    pub device_name: String,
    pub status: Status,
    pub item: Option<Item>,
    pub anchor: Option<Anchor>,
    pub volume: Option<u8>,
    /// Bumped on every `track_changed` and every state with another item.
    pub track_changes: u64,
    /// The last refusal and when it came.
    pub error: Option<(String, Instant)>,
    /// When the last event arrived.
    pub heard: Option<Instant>,
}

impl Live {
    fn apply(&mut self, e: Event) {
        self.heard = Some(Instant::now());
        match e {
            Event::Auth { logged_in, active, device_name } => {
                self.logged_in = logged_in;
                self.active = active;
                if !device_name.is_empty() {
                    self.device_name = device_name;
                }
                if !logged_in {
                    self.status = Status::Idle;
                    self.item = None;
                    self.anchor = None;
                }
            }
            Event::State { status, item, position, volume, active } => {
                self.status = status;
                if item.as_ref().map(|i| &i.uri) != self.item.as_ref().map(|i| &i.uri) {
                    self.track_changes += 1;
                }
                self.item = item;
                if position.is_some() {
                    self.anchor = position;
                }
                if volume.is_some() {
                    self.volume = volume;
                }
                if let Some(a) = active {
                    self.active = a;
                }
            }
            Event::TrackChanged(item) => {
                self.track_changes += 1;
                self.item = item;
            }
            Event::PlaybackChanged(status) => self.status = status,
            Event::Volume(v) => self.volume = Some(v),
            Event::Device { active } => self.active = active,
            Event::Position(a) => self.anchor = Some(a),
            Event::Error(m) => {
                log::info!("soloist refused a command: {m}");
                self.error = Some((m, Instant::now()));
            }
            Event::Accepted(_) | Event::Other(_) => {}
        }
    }

    /// The URI playing now, if any.
    pub fn uri(&self) -> Option<&str> {
        self.item.as_ref().map(|i| i.uri.as_str()).filter(|u| !u.is_empty())
    }
}

pub type Shared = Arc<Mutex<Live>>;

pub fn lock(live: &Shared) -> std::sync::MutexGuard<'_, Live> {
    live.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Connects to `ws://127.0.0.1:port`, asks for the auth state and the
/// playback state, then serves until the socket ends. `changed` is called
/// after every event (the settings poll the state, this wakes them).
pub async fn serve(
    port: u16,
    live: Shared,
    mut commands: mpsc::UnboundedReceiver<Command>,
    changed: &(dyn Fn() + Send + Sync),
) -> Result<(), String> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(4 << 20))
        .max_frame_size(Some(4 << 20));
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async_with_config(format!("ws://127.0.0.1:{port}"), Some(config), false),
    )
    .await
    .map_err(|_| "Soloist API did not answer".to_owned())?
    .map_err(|e| format!("Soloist API: {e}"))?;
    lock(&live).connected = true;
    let result = async {
        for c in [Command::GetAuthState, Command::GetState] {
            socket.send(Message::Text(c.to_json().into())).await.map_err(|e| e.to_string())?;
        }
        loop {
            tokio::select! {
                msg = socket.next() => match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(e) = Event::parse(&text) {
                            lock(&live).apply(e);
                            changed();
                        }
                    }
                    Some(Ok(Message::Ping(p))) => socket.send(Message::Pong(p)).await.map_err(|e| e.to_string())?,
                    Some(Ok(Message::Close(_))) | None => return Err("Soloist closed its API".to_owned()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.to_string()),
                },
                cmd = commands.recv() => match cmd {
                    Some(c) => {
                        log::debug!("soloist: {}", c.name());
                        socket.send(Message::Text(c.to_json().into())).await.map_err(|e| e.to_string())?;
                    }
                    None => return Ok(()),
                },
            }
        }
    }
    .await;
    let _ = socket.close(None).await;
    lock(&live).connected = false;
    changed();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    /// A server that speaks the documented frames: greets with auth_state
    /// and playback_state, answers commands, and plays what it is told.
    async fn mock(listener: TcpListener) -> Vec<Value> {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let send = |v: Value| Message::Text(v.to_string().into());
        ws.send(send(json!({"type":"auth_state","logged_in":true,"is_active":false,"device_name":"PC"}))).await.unwrap();
        let mut got = Vec::new();
        while let Some(Ok(Message::Text(t))) = ws.next().await {
            let v: Value = serde_json::from_str(&t).unwrap();
            assert_eq!(v["type"], "command");
            got.push(v.clone());
            match v["command"].as_str().unwrap() {
                "get_auth_state" => ws.send(send(json!({"type":"auth_state","logged_in":true,"is_active":false,"device_name":"PC"}))).await.unwrap(),
                "get_state" => ws.send(send(json!({"type":"playback_state","status":"idle","is_active":false,"volume":40}))).await.unwrap(),
                "play" => {
                    ws.send(send(json!({"type":"command_result","command":"play"}))).await.unwrap();
                    ws.send(send(json!({"type":"track_changed","item":{"uri":v["uri"],"entity_type":"track","decorations":{"playback":{"duration_ms":200000}}}}))).await.unwrap();
                    ws.send(send(json!({"type":"playback_changed","status":"playing"}))).await.unwrap();
                    ws.send(send(json!({"type":"position_sync","position":{"position_ms":0,"timestamp_ms":1,"speed":1.0}}))).await.unwrap();
                }
                "seek" => ws.send(send(json!({"type":"error","message":"seek not allowed"}))).await.unwrap(),
                _ => {}
            }
            if v["command"] == "pause" {
                break;
            }
        }
        got
    }

    #[tokio::test]
    async fn events_update_the_state_and_commands_go_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(mock(listener));
        let live = Shared::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let l = live.clone();
        let client = tokio::spawn(async move { serve(port, l, rx, &|| {}).await });
        tx.send(Command::Play(Some("spotify:track:abc".into()))).unwrap();
        tx.send(Command::Seek(5000)).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        {
            let l = lock(&live);
            assert!(l.connected && l.logged_in);
            assert_eq!(l.device_name, "PC");
            assert_eq!(l.uri(), Some("spotify:track:abc"));
            assert_eq!(l.status, Status::Playing);
            assert_eq!(l.item.as_ref().unwrap().duration_ms, Some(200_000));
            assert_eq!(l.volume, Some(40));
            assert!(l.anchor.is_some());
            assert_eq!(l.error.as_ref().map(|e| e.0.as_str()), Some("seek not allowed"));
        }
        tx.send(Command::Pause).unwrap();
        let got = server.await.unwrap();
        let names: Vec<_> = got.iter().map(|v| v["command"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["get_auth_state", "get_state", "play", "seek", "pause"]);
        assert_eq!(got[2]["uri"], "spotify:track:abc");
        assert_eq!(got[3]["position_ms"], 5000);
        let ended = client.await.unwrap();
        assert!(ended.is_err(), "the server went away");
        assert!(!lock(&live).connected);
    }
}

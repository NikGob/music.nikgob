//! The few YouTube Music calls RustyPipe has no function for (likes,
//! playlist edits, the account name), sent through its raw request with the
//! login cookies, and the bits of their JSON answers we need.
//!
//! The answers are searched by key rather than by full path, so small layout
//! changes on YouTube's side do not break them.

use rustypipe::client::{ClientType, RustyPipe};
use serde_json::{Value, json};

use crate::map_err;
use nikgob_core::streaming::StreamError;

pub(crate) async fn call(rp: &RustyPipe, endpoint: &str, body: Value) -> Result<Value, StreamError> {
    let text = rp
        .query()
        .authenticated()
        .raw(ClientType::DesktopMusic, endpoint, &body)
        .await
        .map_err(map_err)?;
    serde_json::from_str(&text).map_err(|e| StreamError::Parse(format!("{endpoint}: {e}")))
}

/// What YouTube Music's queue says about `video_id` (one `next` request,
/// as the player asks it): the item and, for a music video, its song (see
/// `counterpart`). With the login when `signed_in`: the counterpart comes
/// with the user's session.
pub(crate) async fn watch(
    rp: &RustyPipe,
    video_id: &str,
    signed_in: bool,
) -> Result<Option<crate::counterpart::Watch>, StreamError> {
    let body = json!({
        "videoId": video_id,
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
    });
    let mut q = rp.query();
    if signed_in {
        q = q.authenticated();
    }
    let text = q
        .raw(ClientType::DesktopMusic, "next", &body)
        .await
        .map_err(map_err)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| StreamError::Parse(format!("next: {e}")))?;
    Ok(crate::counterpart::parse_watch(&v, video_id))
}

/// Display name of the logged-in account.
pub(crate) async fn account_name(rp: &RustyPipe) -> Result<Option<String>, StreamError> {
    let v = call(rp, "account/account_menu", json!({})).await?;
    Ok(find_key(&v, "accountName").and_then(text))
}

pub(crate) async fn set_liked(
    rp: &RustyPipe,
    video_id: &str,
    liked: bool,
) -> Result<(), StreamError> {
    let endpoint = if liked {
        "like/like"
    } else {
        "like/removelike"
    };
    call(rp, endpoint, json!({ "target": { "videoId": video_id } }))
        .await
        .map(drop)
}

/// Creates a private playlist; its id.
pub(crate) async fn create_playlist(rp: &RustyPipe, name: &str) -> Result<String, StreamError> {
    let v = call(
        rp,
        "playlist/create",
        json!({ "title": name, "description": "", "privacyStatus": "PRIVATE" }),
    )
    .await?;
    find_key(&v, "playlistId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| StreamError::Parse("playlist/create: no playlistId".into()))
}

/// Appends `add` and removes every occurrence of `remove`.
pub(crate) async fn edit_playlist(
    rp: &RustyPipe,
    id: &str,
    add: &[String],
    remove: &[String],
) -> Result<(), StreamError> {
    let mut actions: Vec<Value> = Vec::new();
    if !remove.is_empty() {
        for (video, set_video) in playlist_entries(rp, id).await? {
            if remove.contains(&video) {
                actions.push(json!({
                    "action": "ACTION_REMOVE_VIDEO",
                    "removedVideoId": video,
                    "setVideoId": set_video,
                }));
            }
        }
    }
    for video in add {
        actions.push(json!({ "action": "ACTION_ADD_VIDEO", "addedVideoId": video }));
    }
    if actions.is_empty() {
        return Ok(());
    }
    let v = call(
        rp,
        "browse/edit_playlist",
        json!({ "playlistId": id, "actions": actions }),
    )
    .await?;
    match v.get("status").and_then(Value::as_str) {
        Some("STATUS_SUCCEEDED") | None => Ok(()),
        Some(s) => Err(StreamError::Parse(format!("edit_playlist: {s}"))),
    }
}

/// `(videoId, setVideoId)` of every playlist entry (all pages); removing an
/// entry needs its `setVideoId`.
async fn playlist_entries(rp: &RustyPipe, id: &str) -> Result<Vec<(String, String)>, StreamError> {
    let mut out = Vec::new();
    let mut v = call(rp, "browse", json!({ "browseId": format!("VL{id}") })).await?;
    for _ in 0..100 {
        entries(&v, &mut out);
        let Some(token) = continuation(&v) else {
            break;
        };
        v = call(rp, "browse", json!({ "continuation": token })).await?;
    }
    Ok(out)
}

fn entries(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(d) = map.get("playlistItemData") {
                let video = d.get("videoId").and_then(Value::as_str);
                let set = d.get("playlistSetVideoId").and_then(Value::as_str);
                if let (Some(video), Some(set)) = (video, set) {
                    out.push((video.to_owned(), set.to_owned()));
                }
            }
            map.values().for_each(|c| entries(c, out));
        }
        Value::Array(a) => a.iter().for_each(|c| entries(c, out)),
        _ => {}
    }
}

/// The continuation token of a browse answer, if there are more pages.
fn continuation(v: &Value) -> Option<String> {
    find_key(v, "continuationCommand")
        .and_then(|c| c.get("token"))
        .or_else(|| find_key(v, "nextContinuationData").and_then(|c| c.get("continuation")))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The first value under `key`, depth first.
fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|c| find_key(c, key))),
        Value::Array(a) => a.iter().find_map(|c| find_key(c, key)),
        _ => None,
    }
}

/// Text of a `{"runs": [{"text": …}]}` or `{"simpleText": …}` value.
pub(crate) fn text(v: &Value) -> Option<String> {
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return Some(s.to_owned());
    }
    let runs = v.get("runs")?.as_array()?;
    let s: String = runs
        .iter()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .collect();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_name_from_menu() {
        let v = json!({"actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {
        "header": {"activeAccountHeaderRenderer": {
            "accountName": {"runs": [{"text": "Nik"}, {"text": " Gob"}]},
            "channelHandle": {"runs": [{"text": "@nik"}]}
        }}}}}}]});
        assert_eq!(
            find_key(&v, "accountName").and_then(text).as_deref(),
            Some("Nik Gob")
        );
        assert_eq!(text(&json!({"simpleText": "A"})).as_deref(), Some("A"));
    }

    #[test]
    fn playlist_entries_and_continuation() {
        let v = json!({"contents": [
            {"musicResponsiveListItemRenderer": {"playlistItemData": {"videoId": "a", "playlistSetVideoId": "s1"}}},
            {"musicResponsiveListItemRenderer": {"playlistItemData": {"videoId": "b", "playlistSetVideoId": "s2"}}},
            {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "T"}}}}
        ]});
        let mut out = Vec::new();
        entries(&v, &mut out);
        assert_eq!(out, [("a".into(), "s1".into()), ("b".into(), "s2".into())]);
        assert_eq!(continuation(&v).as_deref(), Some("T"));
        let old = json!({"continuations": [{"nextContinuationData": {"continuation": "C"}}]});
        assert_eq!(continuation(&old).as_deref(), Some("C"));
        assert_eq!(continuation(&json!({})), None);
    }
}
